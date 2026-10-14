//! COLLADA (`.dae`) 1.4.1 — read and write.
//!
//! # Who reads what this writes
//!
//! Collada is the interchange format of the pre-glTF tool chain: 3ds Max,
//! Maya, SketchUp, Unity, Godot, SceneKit, assimp and three.js read it (Blender
//! did until 5.0 removed it). The schema can carry nearly the whole scene —
//! node hierarchy, Phong materials with textures, skins, morph targets and
//! keyframed transforms — but every reader honours a different subset, so the
//! writer is shaped by what the readers people actually use do with each
//! construct. The oracles are three.js r183's `ColladaLoader` (the most widely
//! used open reader, and the engine of the dashboard's viewer), Apple's
//! SceneKit importer (the stack behind Quick Look) and assimp 6.0; each
//! decision below names the evidence, and every document this writer
//! produces validates against the 1.4.1 XSD.
//!
//! # Decisions, and why
//!
//! - **Morph targets are baked, never written.** three.js fails the WHOLE load
//!   on any `<morph>` controller: `ColladaParser.parseController` stores a
//!   controller with no `skin` for it ("Morph target animation not supported
//!   yet."), and `ColladaComposer.buildNode` then reads `controller.skin.joints`
//!   ("Cannot read properties of undefined (reading 'joints')"); a skin over a
//!   morph fails in `buildController` reading `sources`. assimp 6.0 cannot
//!   import one either: the `<targets>` loop in `ColladaParser::ReadController`
//!   advances the wrong node, the target id stays empty, and the import fails
//!   with "Unable to resolve library reference ''". A file with morphs would
//!   open in neither, so [`prepare`] applies every target at its node's
//!   default weights, drops the weight animation, and the loss is reported.
//!   Morph controllers ARE read (NORMALIZED and RELATIVE): DCC files carry them.
//!
//! - **One `<geometry>` per primitive.** three.js merges every primitive of a
//!   `<mesh>` into one `BufferGeometry` and concatenates their attributes
//!   (`buildGeometryType`); a primitive without colours or normals beside one
//!   with them shifts every later attribute out of line with its position. A
//!   geometry per primitive, instanced together on the node, cannot misalign.
//!   SceneKit, though, keeps only one `<instance_controller>` per node (a
//!   two-controller node loads with one skinned geometry), so a skinned
//!   mesh's later primitives are drawn from identity child nodes — harmless,
//!   since skinning ignores the holding node's transform. The reader merges a
//!   node's instances back into one mesh, and controllers that bind the same
//!   joints the same way into one skin.
//!
//! - **Vertex colours are linear, and written inside `<vertices>`.** The spec
//!   gives colour no space; the readers decide. SceneKit takes a `COLOR` as
//!   linear wherever it sits (measured: 0.5 renders as sRGB 188), as does
//!   assimp (which also writes glTF's linear colours as-is), and so does
//!   three.js for a `COLOR` inside `<vertices>`. Only
//!   three.js's primitive-level path decodes sRGB, through an off-by-one
//!   (`buildGeometryData`: `startIndex = array.length - sourceStride - 1`
//!   converts the previous vertex's alpha, then R and G, and never B). Written
//!   in `<vertices>`, the scene's linear colours reach all three exactly; read
//!   anywhere, a `COLOR` is linear.
//!
//! - **Material colours are sRGB-encoded.** three.js converts `<diffuse>`,
//!   `<emission>` and `<specular>` colours from sRGB to linear
//!   (`ColladaComposer.buildMaterial`: `ColorManagement.colorSpaceToWorking(
//!   material.color, SRGBColorSpace)`, and `ColorManagement.enabled` is true by
//!   default; measured: 0.5 loads as 0.214), as does SceneKit (0.5 renders as
//!   sRGB 127); assimp passes them through unconverted. Writing the linear
//!   value would show every flat-coloured part washed out in both, so colours
//!   are encoded on write and decoded on read, unclamped, with three.js's own
//!   curve.
//!
//! - **Matrices.** Collada matrices are column-vector math written ROW-major
//!   ("written in row-major order to aid the human reader", spec 1.4.1 §5),
//!   the transpose of the scene's column-major array. Every node is one
//!   `<matrix sid="transform">`; the reader also composes `translate`,
//!   `rotate`, `scale` and `lookat` stacks in document order.
//!
//! - **Animation is one baked matrix channel per animated node,** sampled with
//!   [`pose::sample_pose`] at every source key plus 30 fps, LINEAR. Collada has
//!   no quaternion element, and three.js decomposes matrix keys to TRS and
//!   interpolates those, turning STEP into linear on matrix channels
//!   (`buildMatrixTracks` never applies the sampler's interpolation) — so a
//!   STEP channel is emulated with an extra key just before each step. For
//!   the same decomposition, a scale that is exactly zero, in a key or on a
//!   node, is written as a millionth (`math::nonsingular_scale`): three.js
//!   turns a singular matrix into unit scale, so a hidden part would show
//!   full size.
//!
//! - **Skins** are written the way three.js binds them: skinned meshes hoisted
//!   to identity roots ([`glb::hoist_skinned_meshes`]), `bind_shape_matrix`
//!   identity, a joints `Name_array` of node `sid`s (three.js names a `JOINT`
//!   bone by its sid and matches by name), every joint `type="JOINT"`, and
//!   `<skeleton>` naming each root joint. On read the bind-shape matrix is
//!   folded into the inverse bind matrices, which is the scene's convention.
//!
//! - **Textures are sidecar files** (`textures/<name>.png`, relative
//!   `<init_from>`), PNG and JPEG only. A self-contained document carries no
//!   images: `data:` URIs in `<init_from>` are not something Collada readers
//!   resolve reliably, and a broken reference is worse than a reported loss.
//!
//! # What is read
//!
//! `<triangles>`, `<polylist>`, `<polygons>`, `<trifans>` and `<tristrips>`
//! (n-gons fanned when convex, ear-clipped when not), with any input layout:
//! shared offsets, `<vertices>` indirection, accessor strides and offsets, up
//! to eight texture coordinate sets. Node transforms in document order,
//! `instance_node` and `library_nodes`, skin and morph controllers, the
//! `profile_COMMON` shaders (diffuse colour or texture through the
//! surface/sampler chain, emission, transparency in all four opaque modes),
//! images through the caller's sidecars, animations of whole matrices, single
//! elements, `translate`/`rotate`/`scale` and their components with LINEAR,
//! STEP, BEZIER and HERMITE keys (resampled to TRS), morph-weight animation,
//! and `animation_clip`s. `<unit>` and `<up_axis>` become one correction on the
//! root nodes, so the scene comes out in metres, Y up.
//!
//! # Untrusted input
//!
//! Every count and index in the file is checked against the data actually
//! present before anything is sized from it, arrays are charged to a parse
//! budget, node instancing is bounded and walked with explicit stacks (an
//! `instance_node` cycle is an error, not a hang), and nothing indexes with a
//! value the file chose without proving it in range first.
//!
//! @see <https://www.khronos.org/files/collada_spec_1_4.pdf>

use std::collections::{BTreeMap, BTreeSet, HashMap, HashSet};
use std::fmt::{self, Write as _};
use std::rc::Rc;

use super::glb;
use super::ir::Mesh;
use super::math::{self, Mat4};
use super::package::{self, NoSidecars, Package, PackageFile, Sidecars};
use super::pose;
use super::scene::{
    AlphaMode, AnimPath, Animation, Capabilities, Channel, Filter, Image, ImageFormat, Interpolation,
    Loss, LossKind, Material, MaterialSupport, MorphTarget, Node, Primitive, Sampler, Scene, SceneMesh, Skin,
    SkinWeights, Texture, TextureRef, Transform, Wrap, MAX_ANIMATION_KEYS, MAX_JOINTS,
    MAX_MORPH_TARGETS, MAX_NODES, MAX_SCENE_INDICES, MAX_SCENE_VERTICES,
};
use super::xml::Element;
use super::{MeshError, MeshFormat, SceneRead, WriteOptions};

// ─── constants ──────────────────────────────────────────────────────────────

/// The 1.4.x namespace, which is what this writer declares.
const NS_COLLADA_141: &str = "http://www.collada.org/2005/11/COLLADASchema";

/// How far into the bytes the root element must start. A real document opens
/// with an XML declaration and perhaps a comment banner; anything whose first
/// tag is further away than this is not a Collada file a tool wrote.
const SNIFF_BYTES: usize = 64 * 1024;

/// Texture coordinate sets the writer carries. three.js reads set 0 as `uv`
/// and set 1 as `uv1` and nothing beyond (`parseGeometryPrimitive` keys a set
/// `n > 0` as `TEXCOORD{n}` and `buildGeometryType` handles only `TEXCOORD1`).
pub const UV_SETS: usize = 2;

/// Texture coordinate sets the reader keeps; past this a primitive's extra
/// sets are reported and dropped.
const MAX_READ_UV_SETS: usize = 8;

/// Joint influences kept per vertex (the glTF reader's ceiling too); the
/// strongest survive and the rest are reported.
const MAX_INFLUENCES: usize = 32;

/// Ceiling on numbers parsed from the document's arrays, for the same reason
/// as glb's `MAX_DECODED_VALUES`: every array is bounded by the input, but the
/// budget is what keeps the parse proportionate (each array is parsed once and
/// cached, so references cannot multiply it). 48M is ~192 MB of `f32`.
const MAX_PARSED_VALUES: usize = 48_000_000;

/// Node nesting depth, counting `instance_node` expansion, beyond which the
/// file is refused. Real hierarchies are a few dozen deep; the scene's own
/// algorithms are iterative, so this bounds work, not the stack.
const MAX_NODE_DEPTH: usize = 512;
/// Transform elements on one `<node>`. Exporters write a handful (Maya's
/// pivot stacks run to about a dozen); each animated sample recomposes the
/// whole stack, so without a ceiling one node's stack multiplies every sample.
const MAX_NODE_OPS: usize = 128;
/// Channel references summed over every `<animation_clip>`: a clip resamples
/// each channel it names, so clips sharing one big animation multiply it.
const MAX_CLIP_CHANNELS: usize = 2_000_000;

/// Largest input offset accepted on a primitive `<input>`. Offsets select
/// columns of the interleaved `<p>` list; real files use a handful.
const MAX_INPUT_OFFSET: usize = 64;

/// Polygons up to this size are ear-clipped when concave; larger concave ones
/// are fanned (and reported), so a hostile n-gon cannot cost O(n²).
const MAX_EAR_CLIP: usize = 64;

/// Sampling rate for animation the writer bakes and the reader resamples.
const RESAMPLE_FPS: f32 = 30.0;

/// Samples per resampled curve, whatever its duration: a key at t = 1e9 must
/// not become thirty billion samples.
const MAX_RESAMPLES: usize = 100_000;

/// How long before a STEP key the emulated jump starts, in seconds: well under
/// a frame at any rate a viewer plays, and representable next to any time of
/// a clip shorter than a day.
const STEP_EPSILON: f32 = 1e-4;

/// Phong exponent ceiling; past this the highlight is a point and the
/// roughness it stands for is effectively zero.
const MAX_SHININESS: f32 = 10_000.0;

/// Characters of file content an error message may quote.
const MAX_DETAIL: usize = 300;

const PRIMARY_FILE: &str = "model.dae";
const TEXTURE_DIR: &str = "textures";
const AUTHORING_TOOL: &str = "litegen mesh converter";
/// `<created>` and `<modified>` are required by the schema; a constant keeps
/// the output byte-identical run to run.
const TIMESTAMP: &str = "2000-01-01T00:00:00Z";

// ─── errors and small helpers ───────────────────────────────────────────────

fn not_dae(detail: impl Into<String>) -> MeshError {
    MeshError::NotThisFormat { expected: MeshFormat::Dae, detail: detail.into() }
}

fn malformed(detail: impl fmt::Display) -> MeshError {
    MeshError::Malformed(format!("collada: {}", clip(&detail.to_string(), MAX_DETAIL)))
}

fn unsupported(detail: impl fmt::Display) -> MeshError {
    MeshError::Unsupported(format!("collada: {}", clip(&detail.to_string(), MAX_DETAIL)))
}

/// At most `max` characters of `s`. Ids and names come from the file, and an
/// error message must never quote an unbounded amount of it.
fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        Some((i, _)) => format!("{}…", &s[..i]),
        None => s.to_string(),
    }
}

/// A file-supplied identifier, quoted and bounded for a message.
fn q(s: &str) -> String {
    format!("{:?}", clip(s, 60))
}

/// Children of `e` with this local name, borrowing from the document rather
/// than from the name.
fn kids<'a, 'n>(e: &'a Element, name: &'n str) -> impl Iterator<Item = &'a Element> + 'n
where
    'a: 'n,
{
    e.children.iter().filter(move |c| c.name == name)
}

fn kid<'a>(e: &'a Element, name: &str) -> Option<&'a Element> {
    e.children.iter().find(|c| c.name == name)
}

fn parse_count(v: &str, what: &str) -> Result<usize, MeshError> {
    v.trim().parse::<usize>().map_err(|_| malformed(format!("{what} is {}, which is not a count", q(v))))
}

fn attr_count(e: &Element, name: &str, what: &str) -> Result<Option<usize>, MeshError> {
    e.attr(name).map(|v| parse_count(v, &format!("{what} {name}"))).transpose()
}

fn mat_ptr(e: &Element) -> usize {
    e as *const Element as usize
}

/// sRGB transfer, unclamped, exactly the curve three.js decodes Collada colours
/// with (`Color.setRGB(..., SRGBColorSpace)`): components above 1 (HDR
/// emission) continue the power curve instead of being cut off.
fn srgb_encode(c: f32) -> f32 {
    let c = c as f64;
    (if c <= 0.003_130_8 { c * 12.92 } else { 1.055 * c.powf(1.0 / 2.4) - 0.055 }) as f32
}

fn srgb_decode(c: f32) -> f32 {
    let c = c as f64;
    (if c <= 0.040_45 { c / 12.92 } else { ((c + 0.055) / 1.055).powf(2.4) }) as f32
}

/// Phong exponent for a roughness, the usual Blinn-Phong/GGX correspondence
/// `s = 2/α² − 2` with `α = roughness²`.
fn shininess_from_roughness(r: f32) -> f32 {
    let r = r.clamp(0.0, 1.0) as f64;
    let a2 = r * r * r * r;
    if a2 <= 2.0 / (MAX_SHININESS as f64 + 2.0) {
        return MAX_SHININESS;
    }
    ((2.0 / a2 - 2.0) as f32).clamp(0.0, MAX_SHININESS)
}

/// The inverse of [`shininess_from_roughness`].
fn roughness_from_shininess(s: f32) -> f32 {
    let s = s.clamp(0.0, MAX_SHININESS) as f64;
    (2.0 / (s + 2.0)).powf(0.25) as f32
}

/// Collada's row-major text of a column-major matrix.
fn row_major(m: &Mat4) -> [f64; 16] {
    math::transpose(m)
}

// ─── capabilities, prepare, losses ──────────────────────────────────────────

/// One row of the lossiness table: what a document this writer produces
/// carries (see `MeshFormat::capabilities`).
///
/// - `texture_filters` is false because no reader honours them: three.js's
///   `parseEffectSampler` reads only `<source>`, and assimp's sampler model
///   has no filter fields.
/// - `texture_wrap` is true: the spec's `wrap_s`/`wrap_t` are written, and so
///   is the MAYA `<extra>` on the `<texture>` (`wrapU`/`wrapV`/`mirrorU`/
///   `mirrorV`), which is what both three.js (`getTexture`) and assimp
///   (`ReadSamplerProperties`) actually read. three.js maps `wrapU` to repeat
///   or clamp and has no mirror: a reader limitation, not the file's.
/// - `texture_transforms` is false: profile_COMMON has none, so
///   `pose::degrade` bakes them into the UVs, which renders identically.
pub fn capabilities(opts: WriteOptions) -> Capabilities {
    Capabilities {
        hierarchy: true,
        normals: true,
        uv_sets: UV_SETS,
        vertex_colors: true,
        materials: MaterialSupport::Phong,
        textures: !opts.self_contained,
        texture_filters: false,
        texture_wrap: true,
        texture_transforms: false,
        skinning: true,
        morph_targets: false,
        animation: true,
        multiple_meshes: true,
    }
}

/// The scene reduced to exactly what [`write_scene`] encodes, which is also
/// what [`read_scene`] gives back for it.
///
/// The generic reduction (`pose::degrade`) would bake skins and hierarchy too
/// the moment `morph_targets` is false, which Collada does not need: only the
/// targets go, baked per node at that node's effective weights. Then the
/// Phong-model specifics: a `<diffuse>` is a colour OR a texture, colours pass
/// through sRGB text, roughness through a Phong exponent, and so on —
/// [`reduce_material`] lists them.
pub fn prepare(scene: &Scene, opts: WriteOptions) -> Result<Scene, MeshError> {
    let caps = capabilities(opts);
    let baked = bake_morph_targets(scene);
    let mut s = pose::degrade(&baked, &Capabilities { morph_targets: true, ..caps })?;
    {
        // Only PNG and JPEG travel as sidecars every Collada reader decodes.
        let Scene { materials, textures, images, .. } = &mut s;
        for m in materials.iter_mut() {
            for slot in [
                &mut m.base_color_texture,
                &mut m.emissive_texture,
                &mut m.normal_texture,
                &mut m.metallic_roughness_texture,
                &mut m.occlusion_texture,
            ] {
                let portable = slot
                    .as_ref()
                    .and_then(|r| textures.get(r.texture))
                    .and_then(|t| images.get(t.image))
                    .is_some_and(|im| im.format.is_universal());
                if !portable {
                    *slot = None;
                }
            }
        }
    }
    for p in s.meshes.iter_mut().flat_map(|m| m.primitives.iter_mut()) {
        // Tangents only serve normal maps, which profile_COMMON cannot carry.
        p.tangents = None;
    }
    for m in s.materials.iter_mut() {
        reduce_material(m);
    }
    pose::prune_unused_textures(&mut s);
    pose::nonsingular_scales(&mut s, true);
    s.validate()?;
    Ok(s)
}

/// Every mesh with morph targets replaced, per node, by its geometry at that
/// node's effective weights; weight channels removed. Instancing survives
/// wherever nodes share a mesh AND weights.
fn bake_morph_targets(scene: &Scene) -> Scene {
    let mut s = scene.clone();
    if !scene.meshes.iter().any(|m| m.target_count() > 0) {
        return s;
    }
    let weights = pose::default_pose(scene).weights;
    let mut out: Vec<SceneMesh> = Vec::new();
    let mut plain: Vec<Option<usize>> = vec![None; scene.meshes.len()];
    let mut baked: HashMap<(usize, Vec<u32>), usize> = HashMap::new();
    for ni in 0..s.nodes.len() {
        let Some(mi) = s.nodes[ni].mesh else { continue };
        let Some(mesh) = scene.meshes.get(mi) else { continue };
        let index = if mesh.target_count() == 0 {
            match plain[mi] {
                Some(i) => i,
                None => {
                    out.push(mesh.clone());
                    plain[mi] = Some(out.len() - 1);
                    out.len() - 1
                }
            }
        } else {
            let w = weights.get(ni).cloned().unwrap_or_default();
            let key = (mi, w.iter().map(|x| x.to_bits()).collect::<Vec<u32>>());
            match baked.get(&key) {
                Some(&i) => i,
                None => {
                    out.push(SceneMesh {
                        name: mesh.name.clone(),
                        primitives: mesh
                            .primitives
                            .iter()
                            .map(|p| {
                                let (positions, normals) = pose::morphed(p, &w);
                                Primitive { positions, normals, targets: Vec::new(), ..p.clone() }
                            })
                            .collect(),
                        weights: Vec::new(),
                        target_names: Vec::new(),
                    });
                    baked.insert(key, out.len() - 1);
                    out.len() - 1
                }
            }
        };
        s.nodes[ni].mesh = Some(index);
        s.nodes[ni].weights = None;
    }
    s.meshes = out;
    for a in s.animations.iter_mut() {
        a.channels.retain(|c| c.path != AnimPath::Weights);
    }
    s.animations.retain(|a| !a.channels.is_empty());
    s
}

/// A material as profile_COMMON carries it, which is exactly what the reader
/// rebuilds from what the writer emits for it.
fn reduce_material(m: &mut Material) {
    // <diffuse> holds a colour or a texture, never their product.
    if m.base_color_texture.is_some() {
        m.base_color[0] = 1.0;
        m.base_color[1] = 1.0;
        m.base_color[2] = 1.0;
    } else {
        for c in m.base_color.iter_mut().take(3) {
            *c = srgb_decode(srgb_encode(*c));
        }
    }
    // <emission> likewise; glTF multiplies its texture by the factor, so a
    // zero factor means the texture shows nothing at all.
    match (&m.emissive_texture, m.emissive) {
        (Some(_), [0.0, 0.0, 0.0]) => m.emissive_texture = None,
        (Some(_), _) => m.emissive = [1.0; 3],
        (None, _) => {
            for c in m.emissive.iter_mut() {
                *c = srgb_decode(srgb_encode(*c));
            }
        }
    }
    m.normal_texture = None;
    m.normal_scale = 1.0;
    m.occlusion_texture = None;
    m.occlusion_strength = 1.0;
    m.metallic_roughness_texture = None;
    m.metallic = 0.0;
    m.roughness = if m.roughness >= 1.0 {
        1.0
    } else {
        roughness_from_shininess(shininess_from_roughness(m.roughness))
    };
    m.unlit = false;
    // Collada has transparency but no alpha test.
    match m.alpha_mode {
        AlphaMode::Opaque => m.base_color[3] = 1.0,
        AlphaMode::Mask => {
            if m.base_color_texture.is_some() {
                m.alpha_mode = AlphaMode::Blend;
            } else if m.base_color[3] >= m.alpha_cutoff {
                m.alpha_mode = AlphaMode::Opaque;
                m.base_color[3] = 1.0;
            } else {
                m.alpha_mode = AlphaMode::Blend;
                m.base_color[3] = 0.0;
            }
        }
        AlphaMode::Blend => {
            if m.base_color_texture.is_none() && m.base_color[3] >= 1.0 {
                m.alpha_mode = AlphaMode::Opaque;
                m.base_color[3] = 1.0;
            }
        }
    }
    m.alpha_cutoff = 0.5;
}

/// What a Collada document loses beyond the generic capability table, named.
/// `scene` is the input, before [`prepare`].
pub fn extra_losses(scene: &Scene, opts: WriteOptions) -> Vec<Loss> {
    let mut out = Vec::new();
    let mut push = |kind: LossKind, what: String, why: &str| out.push(Loss::new(kind, what, format!("dae {why}")));
    // Materials matching `f`, named (by scene index when unnamed).
    let pick = |f: &dyn Fn(&Material) -> bool| -> Vec<String> {
        scene
            .materials
            .iter()
            .enumerate()
            .filter(|(_, m)| f(m))
            .map(|(i, m)| m.name.clone().unwrap_or_else(|| format!("#{i}")))
            .collect()
    };
    let weight_channels =
        scene.animations.iter().flat_map(|a| &a.channels).filter(|c| c.path == AnimPath::Weights).count();
    if weight_channels > 0 {
        push(
            LossKind::MorphTargets,
            format!("{weight_channels} morph-weight animation channel(s)"),
            "is written without morph controllers (three.js and assimp fail to load them); the targets are \
             applied at their default weights and their animation cannot follow",
        );
    }
    if !opts.self_contained {
        let tinted = pick(&|m| m.base_color_texture.is_some() && m.base_color[..3] != [1.0; 3]);
        if !tinted.is_empty() {
            push(
                LossKind::Materials,
                format!("base colour factor of {} textured material(s) [{}]", tinted.len(), tinted.join(", ")),
                "has a diffuse colour OR texture, not their product; the texture is kept",
            );
        }
        let glow = pick(&|m| m.emissive_texture.is_some() && m.emissive != [0.0; 3] && m.emissive != [1.0; 3]);
        if !glow.is_empty() {
            push(
                LossKind::Materials,
                format!("emissive factor of {} material(s) [{}]", glow.len(), glow.join(", ")),
                "has an emission colour OR texture, not their product; the texture is kept",
            );
        }
        let normal_maps = scene.materials.iter().filter(|m| m.normal_texture.is_some()).count();
        if normal_maps > 0 {
            push(
                LossKind::Textures,
                format!("{normal_maps} normal map(s)"),
                "profile_COMMON has no normal-map input (`bump` means a height map to most readers)",
            );
        }
        // Texture-transform conflicts are named by `scene::losses` for every
        // container without texture transforms, Collada included.
    }
    let unlit = pick(&|m| m.unlit);
    if !unlit.is_empty() {
        push(
            LossKind::Materials,
            format!("unlit shading of {} material(s) [{}]", unlit.len(), unlit.join(", ")),
            "is written as lambert: a schema-valid <constant> has no diffuse, and three.js renders it white",
        );
    }
    let masked = pick(&|m| m.alpha_mode == AlphaMode::Mask);
    if !masked.is_empty() {
        push(
            LossKind::Alpha,
            format!("alpha cutoff of {} material(s) [{}]", masked.len(), masked.join(", ")),
            "has transparency but no alpha test; MASK is written as blending",
        );
    }
    out
}

// ─── reader: sniffing ───────────────────────────────────────────────────────

fn find(hay: &[u8], needle: &[u8]) -> Option<usize> {
    hay.windows(needle.len()).position(|w| w == needle)
}

/// The local name of the document's root element, found by skipping the XML
/// prolog by hand: deciding "is this Collada at all" must not depend on the
/// whole file parsing, or a truncated Collada file would be misreported as
/// some other format.
fn root_name(bytes: &[u8]) -> Option<String> {
    let head = &bytes[..bytes.len().min(SNIFF_BYTES)];
    let mut i = if head.starts_with(&[0xEF, 0xBB, 0xBF]) { 3 } else { 0 };
    loop {
        while head.get(i).is_some_and(|b| b.is_ascii_whitespace()) {
            i += 1;
        }
        let rest = head.get(i..)?;
        if rest.starts_with(b"<?") {
            i += find(rest, b"?>")? + 2;
        } else if rest.starts_with(b"<!--") {
            i += find(rest, b"-->")? + 3;
        } else if rest.starts_with(b"<!") {
            // DOCTYPE, whose internal subset may itself contain '>'.
            let mut depth = 0usize;
            let mut j = 2;
            loop {
                match *rest.get(j)? {
                    b'[' => depth += 1,
                    b']' => depth = depth.saturating_sub(1),
                    b'>' if depth == 0 => break,
                    _ => {}
                }
                j += 1;
            }
            i += j + 1;
        } else if rest.first() == Some(&b'<') {
            let name: Vec<u8> = rest[1..]
                .iter()
                .take_while(|b| !b.is_ascii_whitespace() && **b != b'/' && **b != b'>')
                .take(256)
                .copied()
                .collect();
            let name = String::from_utf8(name).ok()?;
            return Some(name.rsplit(':').next().unwrap_or("").to_string());
        } else {
            return None;
        }
    }
}

fn sniff(bytes: &[u8]) -> Result<(), MeshError> {
    if bytes.starts_with(&[0xFE, 0xFF]) || bytes.starts_with(&[0xFF, 0xFE]) {
        return Err(not_dae("the bytes are UTF-16 text; COLLADA documents are read as UTF-8"));
    }
    match root_name(bytes) {
        Some(n) if n == "COLLADA" => Ok(()),
        Some(n) => Err(not_dae(format!("the XML root element is <{}>, not <COLLADA>", clip(&n, 60)))),
        None => Err(not_dae("not an XML document whose root element is <COLLADA>")),
    }
}

// ─── reader: document index ─────────────────────────────────────────────────

struct Doc<'a> {
    root: &'a Element,
    ids: HashMap<&'a str, &'a Element>,
    duplicate_ids: usize,
}

impl<'a> Doc<'a> {
    fn new(root: &'a Element) -> Doc<'a> {
        let mut ids = HashMap::new();
        let mut duplicate_ids = 0;
        let mut stack = vec![root];
        while let Some(e) = stack.pop() {
            if let Some(id) = e.attr("id") {
                // XML ids are unique; when a file repeats one, the first in
                // document order is the one a reader resolving `#id` meets.
                if ids.contains_key(id) {
                    duplicate_ids += 1;
                } else {
                    ids.insert(id, e);
                }
            }
            stack.extend(e.children.iter().rev());
        }
        Doc { root, ids, duplicate_ids }
    }

    /// The element a `#id` URL names. References into other documents
    /// (`other.dae#id`) are not followed: conversion has no filesystem.
    fn get(&self, url: &str) -> Option<&'a Element> {
        let id = url.trim().strip_prefix('#')?;
        self.ids.get(id).copied()
    }

    fn by_id(&self, id: &str) -> Option<&'a Element> {
        self.ids.get(id).copied()
    }

    /// Every `item` of every `<library_...>` called `lib`.
    fn library(&self, lib: &str, item: &str) -> Vec<&'a Element> {
        kids(self.root, lib).flat_map(|l| kids(l, item).collect::<Vec<_>>()).collect()
    }

    fn resolve(&self, url: &str, expect: &str, at: &str) -> Result<&'a Element, MeshError> {
        let e = match self.get(url) {
            Some(e) => e,
            None if !url.trim().starts_with('#') => {
                return Err(unsupported(format!(
                    "{at} refers to {}, outside this document; only #id references within the file are followed",
                    q(url)
                )))
            }
            None => return Err(malformed(format!("{at} refers to {}, which is not in the document", q(url)))),
        };
        if e.name != expect {
            return Err(malformed(format!("{at} refers to {}, a <{}> where a <{expect}> belongs", q(url), clip(&e.name, 40))));
        }
        Ok(e)
    }
}

// ─── reader: sources and accessors ──────────────────────────────────────────

/// An accessor resolved and checked: element `i`'s component `c` is
/// `array[offset + i * stride + comps[c]]`, for every `i < count`, proven in
/// range by [`View::check`] before anything reads it.
struct View<'a> {
    source_id: &'a str,
    array: &'a Element,
    count: usize,
    offset: usize,
    stride: usize,
    /// Positions within an element of the NAMED params, expanded by width; an
    /// unnamed `<param>` is, per the spec, a value to skip.
    comps: Vec<usize>,
}

impl View<'_> {
    fn check(&self, len: usize) -> Result<(), MeshError> {
        if self.count == 0 {
            return Ok(());
        }
        let last = (self.count - 1)
            .checked_mul(self.stride)
            .and_then(|x| x.checked_add(self.offset))
            .and_then(|x| x.checked_add(self.comps.iter().copied().max().unwrap_or(0)));
        match last {
            Some(l) if l < len => Ok(()),
            _ => Err(malformed(format!(
                "source {} describes {} elements of stride {} from offset {}, past its {len}-value array",
                q(self.source_id),
                self.count,
                self.stride,
                self.offset
            ))),
        }
    }

    fn at(&self, i: usize, c: usize) -> usize {
        self.offset + i * self.stride + self.comps[c]
    }
}

fn param_width(ty: &str) -> usize {
    match ty {
        "float4x4" => 16,
        "float3x3" => 9,
        "float4" | "float2x2" => 4,
        "float3" => 3,
        "float2" => 2,
        _ => 1,
    }
}

fn view_of<'a>(doc: &Doc<'a>, source: &'a Element) -> Result<View<'a>, MeshError> {
    let sid = source.attr("id").unwrap_or("");
    if source.name != "source" {
        return Err(malformed(format!("{} is a <{}>, not a <source>", q(sid), clip(&source.name, 40))));
    }
    let acc = kid(source, "technique_common")
        .and_then(|t| kid(t, "accessor"))
        .ok_or_else(|| malformed(format!("source {} has no technique_common accessor", q(sid))))?;
    let at = format!("source {} accessor", q(sid));
    let count = attr_count(acc, "count", &at)?.ok_or_else(|| malformed(format!("{at} has no count")))?;
    let offset = attr_count(acc, "offset", &at)?.unwrap_or(0);
    let stride = attr_count(acc, "stride", &at)?.unwrap_or(1);
    if stride == 0 {
        return Err(malformed(format!("{at} has stride 0")));
    }
    let array = match acc.attr("source") {
        Some(url) => doc.get(url).ok_or_else(|| malformed(format!("{at} names {}, which is not in the document", q(url))))?,
        // The accessor's `source` is required; the array beside it is the
        // only thing it could have meant.
        None => source
            .children
            .iter()
            .find(|c| c.name.ends_with("_array"))
            .ok_or_else(|| malformed(format!("{at} names no array")))?,
    };
    if !matches!(array.name.as_str(), "float_array" | "int_array" | "Name_array" | "IDREF_array" | "bool_array") {
        return Err(malformed(format!("{at} names a <{}>, not an array", clip(&array.name, 40))));
    }
    let mut comps = Vec::new();
    let mut pos = 0usize;
    let params: Vec<&Element> = kids(acc, "param").collect();
    for p in &params {
        let width = param_width(p.attr("type").unwrap_or("float"));
        if p.attr("name").is_some_and(|n| !n.is_empty()) {
            comps.extend(pos..pos + width);
        }
        pos += width;
    }
    if params.is_empty() {
        // No params at all: the whole stride is the element.
        comps.extend(0..stride);
        pos = stride;
    }
    if pos > stride {
        return Err(malformed(format!("{at} params span {pos} values but the stride is {stride}")));
    }
    Ok(View { source_id: sid, array, count, offset, stride, comps })
}

// ─── reader: state ──────────────────────────────────────────────────────────

#[derive(Clone)]
struct GeomPrim {
    symbol: Option<String>,
    positions: Vec<[f32; 3]>,
    normals: Option<Vec<[f32; 3]>>,
    uvs: Vec<Vec<[f32; 2]>>,
    /// The Collada `set` of each entry of `uvs`.
    uv_sets: Vec<u32>,
    colors: Option<Vec<[f32; 4]>>,
    /// Each vertex's index into `<vertices>`, which is what skin weights and
    /// morph targets are indexed by.
    vertex: Vec<u32>,
    indices: Vec<u32>,
}

struct GeomData {
    name: Option<String>,
    prims: Vec<GeomPrim>,
    /// Elements of `<vertices>` (its POSITION source).
    vertex_count: usize,
    /// Maya's `<extra><technique profile="MAYA"><double_sided>` on the geometry.
    double_sided: bool,
}

struct SkinData<'a> {
    name: Option<String>,
    geometry: &'a Element,
    morph: Option<Rc<MorphData<'a>>>,
    joints: Vec<String>,
    /// The joint source is an `IDREF_array` (node ids) rather than sids.
    joints_are_ids: bool,
    /// INV_BIND_MATRIX × bind_shape_matrix, column-major: the scene's IBM.
    ibms: Vec<Mat4>,
    bind_shape: Mat4,
    /// Influences of `<vertices>` element `v`: `pairs[starts[v]..starts[v + 1]]`,
    /// joint `-1` meaning the bind shape itself.
    starts: Vec<usize>,
    pairs: Vec<(i64, f32)>,
}

struct MorphData<'a> {
    /// The `<controller>` holding the morph: what weight channels address.
    controller: usize,
    base: &'a Element,
    relative: bool,
    targets: Vec<&'a Element>,
    names: Vec<String>,
    weights: Vec<f32>,
}

enum Controller<'a> {
    Skin(Rc<SkinData<'a>>),
    Morph(Rc<MorphData<'a>>),
}

#[derive(Clone)]
enum TexSource {
    Color([f32; 4]),
    Texture { texture: usize, image: usize, texcoord: String },
}

#[derive(Clone, Copy, PartialEq)]
enum Shading {
    Constant,
    Lambert,
    Phong,
}

#[derive(Clone)]
struct EffectData {
    shading: Shading,
    emission: Option<TexSource>,
    diffuse: Option<TexSource>,
    transparent: Option<(String, TexSource)>,
    transparency: Option<f32>,
    shininess: Option<f32>,
    double_sided: bool,
}

/// One `<instance_material>`: symbol, material, and the texcoord semantics it
/// binds to `TEXCOORD` sets.
type MaterialBinding<'a> = (&'a str, &'a Element, Vec<(String, u32)>);

/// A `<bind_material>`: material symbol → material, with the texcoord
/// bindings of each.
struct Binding<'a> {
    entries: Vec<MaterialBinding<'a>>,
    /// A canonical spelling, so two instances that bind identically share
    /// one mesh.
    signature: String,
}

enum PartSource<'a> {
    Geometry(&'a Element),
    Controller(&'a Element),
}

struct Part<'a> {
    source: PartSource<'a>,
    binding: Rc<Binding<'a>>,
    skeletons: Vec<String>,
}

#[derive(Clone, Copy, PartialEq, Debug)]
enum OpKind {
    Matrix,
    Translate,
    Rotate,
    Scale,
    LookAt,
}

#[derive(Clone)]
struct Op {
    sid: Option<String>,
    kind: OpKind,
    /// As written: a matrix row-major, a rotate as axis then degrees.
    values: Vec<f64>,
}

struct NodeInfo<'a> {
    /// Shared by every `instance_node` copy of the same element: parsed once.
    ops: Rc<Vec<Op>>,
    parts: Vec<Part<'a>>,
    sid: Option<&'a str>,
    id: Option<&'a str>,
    name: Option<&'a str>,
}

struct Reader<'a> {
    doc: Doc<'a>,
    sidecars: &'a dyn Sidecars,
    scene: Scene,
    warnings: Vec<String>,
    warned: HashSet<String>,
    parsed: usize,
    total_vertices: usize,
    total_indices: usize,
    total_keys: usize,
    correction: Option<Mat4>,
    f32_cache: HashMap<usize, Rc<Vec<f32>>>,
    f64_cache: HashMap<usize, Rc<Vec<f64>>>,
    name_cache: HashMap<usize, Rc<Vec<String>>>,
    geom_cache: HashMap<usize, Rc<GeomData>>,
    controller_cache: HashMap<usize, Rc<Controller<'a>>>,
    effect_cache: HashMap<usize, Rc<EffectData>>,
    image_cache: HashMap<usize, Option<usize>>,
    texture_cache: HashMap<(usize, String), usize>,
    material_cache: HashMap<String, Option<usize>>,
    mesh_cache: HashMap<String, usize>,
    skin_cache: HashMap<String, usize>,
    info: Vec<NodeInfo<'a>>,
    nodes_by_element: HashMap<usize, Vec<usize>>,
    /// Each `<node>` element's transform stack, parsed once however many
    /// `instance_node` copies re-enter it.
    ops_by_element: HashMap<usize, Rc<Vec<Op>>>,
    /// Morph controllers → the scene nodes that draw them, for weight channels.
    morph_nodes: HashMap<usize, Vec<usize>>,
    /// MORPH_WEIGHT `<source>`s and their arrays → (morph controller, target
    /// count), so weight channels resolve without a library scan each.
    weight_arrays: Option<HashMap<usize, (usize, usize)>>,
    /// Which nodes are roots (the unit/up-axis correction applies to them).
    is_root: Vec<bool>,
    /// Lookup tables for joint resolution, built on first use.
    global_sid: Option<HashMap<String, usize>>,
    global_id: Option<HashMap<String, usize>>,
    global_name: Option<HashMap<String, usize>>,
    subtree_sid: HashMap<Vec<usize>, Rc<HashMap<String, usize>>>,
}

impl<'a> Reader<'a> {
    fn new(doc: Doc<'a>, sidecars: &'a dyn Sidecars) -> Reader<'a> {
        Reader {
            doc,
            sidecars,
            scene: Scene::default(),
            warnings: Vec::new(),
            warned: HashSet::new(),
            parsed: 0,
            total_vertices: 0,
            total_indices: 0,
            total_keys: 0,
            correction: None,
            f32_cache: HashMap::new(),
            f64_cache: HashMap::new(),
            name_cache: HashMap::new(),
            geom_cache: HashMap::new(),
            controller_cache: HashMap::new(),
            effect_cache: HashMap::new(),
            image_cache: HashMap::new(),
            texture_cache: HashMap::new(),
            material_cache: HashMap::new(),
            mesh_cache: HashMap::new(),
            skin_cache: HashMap::new(),
            info: Vec::new(),
            nodes_by_element: HashMap::new(),
            ops_by_element: HashMap::new(),
            morph_nodes: HashMap::new(),
            weight_arrays: None,
            is_root: Vec::new(),
            global_sid: None,
            global_id: None,
            global_name: None,
            subtree_sid: HashMap::new(),
        }
    }

    fn warn(&mut self, w: String) {
        self.warnings.push(clip(&w, MAX_DETAIL));
    }

    /// Warn once per `key`, so a property repeated on every primitive of a
    /// large file does not bury every other warning.
    fn warn_once(&mut self, key: &str, w: String) {
        if self.warned.insert(key.to_string()) {
            self.warn(w);
        }
    }

    // ── arrays ──────────────────────────────────────────────────────────────

    fn charge(&mut self, n: usize, what: &str) -> Result<(), MeshError> {
        self.parsed = self.parsed.saturating_add(n);
        if self.parsed > MAX_PARSED_VALUES {
            return Err(unsupported(format!(
                "parsing {what} takes the document past {MAX_PARSED_VALUES} numbers; it describes more data than \
                 this converter will hold"
            )));
        }
        Ok(())
    }

    /// Token count of an array element, checked against its `count`.
    fn tokens(&mut self, arr: &Element) -> Result<usize, MeshError> {
        let n = arr.text.split_ascii_whitespace().count();
        let id = arr.attr("id").unwrap_or("");
        if let Some(declared) = attr_count(arr, "count", &format!("<{}> {}", clip(&arr.name, 20), q(id)))? {
            if declared != n {
                return Err(malformed(format!(
                    "<{}> {} declares {declared} values and holds {n}",
                    clip(&arr.name, 20),
                    q(id)
                )));
            }
        }
        self.charge(n, &format!("array {}", q(id)))?;
        Ok(n)
    }

    fn floats_f32(&mut self, arr: &'a Element) -> Result<Rc<Vec<f32>>, MeshError> {
        if let Some(v) = self.f32_cache.get(&mat_ptr(arr)) {
            return Ok(v.clone());
        }
        if arr.name != "float_array" {
            return Err(malformed(format!("{} is a <{}>, not a float_array", q(arr.attr("id").unwrap_or("")), clip(&arr.name, 20))));
        }
        let n = self.tokens(arr)?;
        let mut out = Vec::with_capacity(n);
        for t in arr.text.split_ascii_whitespace() {
            out.push(
                t.parse::<f32>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| malformed(format!("float_array {} holds {}, not a finite number", q(arr.attr("id").unwrap_or("")), q(t))))?,
            );
        }
        let rc = Rc::new(out);
        self.f32_cache.insert(mat_ptr(arr), rc.clone());
        Ok(rc)
    }

    fn floats_f64(&mut self, arr: &'a Element) -> Result<Rc<Vec<f64>>, MeshError> {
        if let Some(v) = self.f64_cache.get(&mat_ptr(arr)) {
            return Ok(v.clone());
        }
        if arr.name != "float_array" {
            return Err(malformed(format!("{} is a <{}>, not a float_array", q(arr.attr("id").unwrap_or("")), clip(&arr.name, 20))));
        }
        let n = self.tokens(arr)?;
        let mut out = Vec::with_capacity(n);
        for t in arr.text.split_ascii_whitespace() {
            out.push(
                t.parse::<f64>()
                    .ok()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| malformed(format!("float_array {} holds {}, not a finite number", q(arr.attr("id").unwrap_or("")), q(t))))?,
            );
        }
        let rc = Rc::new(out);
        self.f64_cache.insert(mat_ptr(arr), rc.clone());
        Ok(rc)
    }

    fn names(&mut self, arr: &'a Element) -> Result<Rc<Vec<String>>, MeshError> {
        if let Some(v) = self.name_cache.get(&mat_ptr(arr)) {
            return Ok(v.clone());
        }
        if !matches!(arr.name.as_str(), "Name_array" | "IDREF_array") {
            return Err(malformed(format!("{} is a <{}>, not a Name_array", q(arr.attr("id").unwrap_or("")), clip(&arr.name, 20))));
        }
        self.tokens(arr)?;
        let rc = Rc::new(arr.text.split_ascii_whitespace().map(str::to_string).collect::<Vec<_>>());
        self.name_cache.insert(mat_ptr(arr), rc.clone());
        Ok(rc)
    }

    /// Non-negative integers (`<p>`, `<vcount>`).
    fn indices(&mut self, e: &Element, what: &str) -> Result<Vec<u32>, MeshError> {
        let n = e.text.split_ascii_whitespace().count();
        self.charge(n, what)?;
        let mut out = Vec::with_capacity(n);
        for t in e.text.split_ascii_whitespace() {
            out.push(t.parse::<u32>().map_err(|_| malformed(format!("{what} holds {}, not an index", q(t))))?);
        }
        Ok(out)
    }

    /// Signed integers (`<v>`, where -1 names the bind shape).
    fn signed(&mut self, e: &Element, what: &str) -> Result<Vec<i64>, MeshError> {
        let n = e.text.split_ascii_whitespace().count();
        self.charge(n, what)?;
        let mut out = Vec::with_capacity(n);
        for t in e.text.split_ascii_whitespace() {
            out.push(t.parse::<i64>().map_err(|_| malformed(format!("{what} holds {}, not an integer", q(t))))?);
        }
        Ok(out)
    }

    /// A float source, its accessor proven against its array, with at least
    /// `need` named components.
    fn stream_f32(&mut self, source: &'a Element, need: usize, what: &str) -> Result<(Rc<Vec<f32>>, View<'a>), MeshError> {
        let v = view_of(&self.doc, source)?;
        if v.comps.len() < need {
            return Err(malformed(format!("{what} source {} has {} components, {need} needed", q(v.source_id), v.comps.len())));
        }
        let vals = self.floats_f32(v.array)?;
        v.check(vals.len())?;
        Ok((vals, v))
    }

    fn stream_f64(&mut self, source: &'a Element, need: usize, what: &str) -> Result<(Rc<Vec<f64>>, View<'a>), MeshError> {
        let v = view_of(&self.doc, source)?;
        if v.comps.len() < need {
            return Err(malformed(format!("{what} source {} has {} components, {need} needed", q(v.source_id), v.comps.len())));
        }
        let vals = self.floats_f64(v.array)?;
        v.check(vals.len())?;
        Ok((vals, v))
    }

    fn stream_names(&mut self, source: &'a Element, what: &str) -> Result<(Rc<Vec<String>>, View<'a>), MeshError> {
        let v = view_of(&self.doc, source)?;
        if v.comps.is_empty() {
            return Err(malformed(format!("{what} source {} has no components", q(v.source_id))));
        }
        let vals = self.names(v.array)?;
        v.check(vals.len())?;
        Ok((vals, v))
    }

    // ── geometry ────────────────────────────────────────────────────────────

    fn geometry(&mut self, geom: &'a Element) -> Result<Rc<GeomData>, MeshError> {
        if let Some(g) = self.geom_cache.get(&mat_ptr(geom)) {
            return Ok(g.clone());
        }
        let gid = geom.attr("id").unwrap_or("");
        let mut data = GeomData {
            name: geom.attr("name").filter(|s| !s.is_empty()).map(str::to_string),
            prims: Vec::new(),
            vertex_count: 0,
            double_sided: double_sided_extra(geom),
        };
        match kid(geom, "mesh") {
            None => {
                let kind = geom.children.iter().find(|c| c.name != "asset" && c.name != "extra").map_or("nothing", |c| c.name.as_str());
                self.warn(format!("geometry {} is a <{}>, not a polygon mesh; it was skipped", q(gid), clip(kind, 30)));
            }
            Some(mesh) => {
                let vertices = kid(mesh, "vertices");
                if let Some(v) = vertices {
                    data.vertex_count = self.vertex_count(gid, v)?;
                }
                for prim in &mesh.children {
                    let kind = prim.name.as_str();
                    match kind {
                        "triangles" | "polylist" | "polygons" | "trifans" | "tristrips" => {}
                        "lines" | "linestrips" => {
                            self.warn_once(
                                "lines",
                                format!("geometry {} has <{kind}>; lines are not triangles and were skipped", q(gid)),
                            );
                            continue;
                        }
                        _ => continue,
                    }
                    let vertices = vertices
                        .ok_or_else(|| malformed(format!("geometry {} has primitives but no <vertices>", q(gid))))?;
                    if let Some(p) = self.primitive(gid, prim, vertices)? {
                        data.prims.push(p);
                    }
                }
            }
        }
        let rc = Rc::new(data);
        self.geom_cache.insert(mat_ptr(geom), rc.clone());
        Ok(rc)
    }

    /// Elements of a `<vertices>`: the count of its POSITION source.
    fn vertex_count(&mut self, gid: &str, vertices: &'a Element) -> Result<usize, MeshError> {
        let pos = kids(vertices, "input")
            .find(|i| i.attr("semantic") == Some("POSITION"))
            .ok_or_else(|| malformed(format!("geometry {} <vertices> has no POSITION input", q(gid))))?;
        let url = pos.attr("source").unwrap_or("");
        let src = self.doc.resolve(url, "source", &format!("geometry {} POSITION", q(gid)))?;
        let (_, v) = self.stream_f32(src, 3, "POSITION")?;
        Ok(v.count)
    }

    fn primitive(&mut self, gid: &str, prim: &'a Element, vertices: &'a Element) -> Result<Option<GeomPrim>, MeshError> {
        let kind = prim.name.as_str();
        let at = format!("geometry {} <{kind}>", q(gid));
        struct In<'a> {
            semantic: &'a str,
            source: &'a Element,
            offset: usize,
            set: u32,
            in_vertices: bool,
        }
        let mut ins: Vec<In<'a>> = Vec::new();
        let mut stride = 0usize;
        for inp in kids(prim, "input") {
            let semantic = inp.attr("semantic").ok_or_else(|| malformed(format!("{at} has an <input> without a semantic")))?;
            let offset = attr_count(inp, "offset", &at)?
                .ok_or_else(|| malformed(format!("{at} input {semantic} has no offset")))?;
            if offset > MAX_INPUT_OFFSET {
                return Err(malformed(format!("{at} input {semantic} has offset {offset}")));
            }
            stride = stride.max(offset + 1);
            let set = match inp.attr("set") {
                None => 0,
                Some(s) => s.trim().parse::<u32>().map_err(|_| malformed(format!("{at} input {semantic} set {} is not a number", q(s))))?,
            };
            let url = inp.attr("source").ok_or_else(|| malformed(format!("{at} input {semantic} has no source")))?;
            if semantic == "VERTEX" {
                let target = self.doc.get(url).ok_or_else(|| malformed(format!("{at} VERTEX names {}, which is not in the document", q(url))))?;
                if !std::ptr::eq(target, vertices) {
                    return Err(malformed(format!("{at} VERTEX names {}, not its mesh's <vertices>", q(url))));
                }
                for vi in kids(vertices, "input") {
                    let vsem = vi.attr("semantic").ok_or_else(|| malformed(format!("{at} <vertices> input without a semantic")))?;
                    let vurl = vi.attr("source").unwrap_or("");
                    let src = self.doc.resolve(vurl, "source", &format!("{at} <vertices> {vsem}"))?;
                    ins.push(In { semantic: vsem, source: src, offset, set: 0, in_vertices: true });
                }
            } else {
                let src = self.doc.resolve(url, "source", &format!("{at} input {semantic}"))?;
                ins.push(In { semantic, source: src, offset, set, in_vertices: false });
            }
        }
        if stride == 0 {
            return Err(malformed(format!("{at} has no inputs")));
        }
        let pos = ins
            .iter()
            .find(|i| i.semantic == "POSITION")
            .ok_or_else(|| malformed(format!("{at} has no POSITION (VERTEX) input")))?;
        if !pos.in_vertices {
            return Err(malformed(format!("{at} takes POSITION outside <vertices>, where skins and morphs cannot index it")));
        }
        let nrm = ins.iter().find(|i| i.semantic == "NORMAL");
        let mut tex: Vec<&In> = Vec::new();
        for i in ins.iter().filter(|i| i.semantic == "TEXCOORD") {
            if !tex.iter().any(|t| t.set == i.set) {
                tex.push(i);
            }
        }
        tex.sort_by_key(|t| t.set);
        if tex.len() > MAX_READ_UV_SETS {
            self.warn_once("uvsets", format!("{at} has {} texture coordinate sets; the first {MAX_READ_UV_SETS} were kept", tex.len()));
            tex.truncate(MAX_READ_UV_SETS);
        }
        let col = ins.iter().filter(|i| i.semantic == "COLOR").min_by_key(|i| i.set);
        for i in &ins {
            if !matches!(i.semantic, "POSITION" | "NORMAL" | "TEXCOORD" | "COLOR") {
                self.warn_once(
                    &format!("input {}", i.semantic),
                    format!("vertex input {} is not carried (it is derivable or not modelled)", q(i.semantic)),
                );
            }
        }

        // Streams, each proven in range of its array.
        let (pv, pview) = self.stream_f32(pos.source, 3, "POSITION")?;
        let nrm_s = match nrm {
            Some(n) => Some((n.offset, self.stream_f32(n.source, 3, "NORMAL")?)),
            None => None,
        };
        let mut tex_s = Vec::with_capacity(tex.len());
        for t in &tex {
            tex_s.push((t.offset, t.set, self.stream_f64(t.source, 2, "TEXCOORD")?));
        }
        let col_s = match col {
            Some(c) => Some((c.offset, self.stream_f32(c.source, 3, "COLOR")?)),
            None => None,
        };

        // Corners and their triangulation.
        let (p, tris) = self.corners(prim, &at, stride, pos.offset, &pv, &pview)?;
        if tris.is_empty() {
            return Ok(None);
        }

        // One scene vertex per distinct combination of indices.
        let corners = tris.len().saturating_mul(3);
        if corners > MAX_SCENE_INDICES {
            return Err(unsupported(format!("{at} has {} triangles, over the scene's limit", tris.len())));
        }
        let mut out = GeomPrim {
            symbol: prim.attr("material").map(str::to_string),
            positions: Vec::new(),
            normals: nrm_s.as_ref().map(|_| Vec::new()),
            uvs: vec![Vec::new(); tex_s.len()],
            uv_sets: tex_s.iter().map(|(_, set, _)| *set).collect(),
            colors: col_s.as_ref().map(|_| Vec::new()),
            vertex: Vec::new(),
            indices: Vec::with_capacity(corners),
        };
        // Distinct index tuples, then numbered in tuple order: a file whose
        // inputs share one index (this writer's, and most) gets its vertices
        // back in their own order, and any file gets a deterministic one.
        let mut map: HashMap<[u32; 12], u32> = HashMap::new();
        let mut uniques: Vec<[u32; 12]> = Vec::new();
        let mut corner_ids: Vec<u32> = Vec::with_capacity(corners);
        for tri in &tris {
            for &c in tri {
                let base = c * stride;
                let mut key = [0u32; 12];
                key[0] = p[base + pos.offset];
                let mut k = 1;
                if let Some((off, _)) = &nrm_s {
                    key[k] = p[base + off];
                    k += 1;
                }
                for (off, _, _) in &tex_s {
                    key[k] = p[base + off];
                    k += 1;
                }
                if let Some((off, _)) = &col_s {
                    key[k] = p[base + off];
                }
                let id = match map.get(&key) {
                    Some(&id) => id,
                    None => {
                        let id = u32::try_from(uniques.len()).map_err(|_| unsupported(format!("{at} has too many vertices")))?;
                        uniques.push(key);
                        map.insert(key, id);
                        id
                    }
                };
                corner_ids.push(id);
            }
        }
        drop(map);
        let mut order: Vec<u32> = (0..uniques.len() as u32).collect();
        order.sort_unstable_by_key(|&i| uniques[i as usize]);
        let mut remap = vec![0u32; uniques.len()];
        for (new, &old) in order.iter().enumerate() {
            remap[old as usize] = new as u32;
        }
        for &old in &order {
            let key = uniques[old as usize];
            let pi = key[0] as usize;
            if pi >= pview.count {
                return Err(malformed(format!("{at} position index {pi} is out of range for {} positions", pview.count)));
            }
            out.positions.push([pv[pview.at(pi, 0)], pv[pview.at(pi, 1)], pv[pview.at(pi, 2)]]);
            out.vertex.push(key[0]);
            let mut k = 1;
            if let (Some((_, (nv, nview))), Some(normals)) = (&nrm_s, out.normals.as_mut()) {
                let ni = key[k] as usize;
                k += 1;
                if ni >= nview.count {
                    return Err(malformed(format!("{at} normal index {ni} is out of range for {} normals", nview.count)));
                }
                normals.push([nv[nview.at(ni, 0)], nv[nview.at(ni, 1)], nv[nview.at(ni, 2)]]);
            }
            for (si, (_, _, (tv, tview))) in tex_s.iter().enumerate() {
                let ti = key[k] as usize;
                k += 1;
                if ti >= tview.count {
                    return Err(malformed(format!("{at} texcoord index {ti} is out of range for {} coordinates", tview.count)));
                }
                // Collada's V runs up the image, the scene's down. The flip is
                // done in f64 so `1 - (1 - v)` gives v back exactly when this
                // writer's file is read.
                out.uvs[si].push([tv[tview.at(ti, 0)] as f32, (1.0 - tv[tview.at(ti, 1)]) as f32]);
            }
            if let (Some((_, (cv, cview))), Some(colors)) = (&col_s, out.colors.as_mut()) {
                let ci = key[k] as usize;
                if ci >= cview.count {
                    return Err(malformed(format!("{at} colour index {ci} is out of range for {} colours", cview.count)));
                }
                let mut c = [cv[cview.at(ci, 0)], cv[cview.at(ci, 1)], cv[cview.at(ci, 2)], 1.0];
                if cview.comps.len() >= 4 {
                    c[3] = cv[cview.at(ci, 3)];
                }
                // Linear, wherever it sits (see the module docs).
                colors.push(c);
            }
        }
        out.indices = corner_ids.into_iter().map(|id| remap[id as usize]).collect();
        Ok(Some(out))
    }

    /// The `<p>` index list of a primitive element (all of its `<p>`s joined)
    /// and its triangles as corner numbers, triangulated by kind.
    #[allow(clippy::type_complexity)]
    fn corners(
        &mut self,
        prim: &'a Element,
        at: &str,
        stride: usize,
        pos_offset: usize,
        pv: &[f32],
        pview: &View<'a>,
    ) -> Result<(Vec<u32>, Vec<[usize; 3]>), MeshError> {
        let kind = prim.name.as_str();
        let declared = attr_count(prim, "count", at)?;
        let mut tris: Vec<[usize; 3]> = Vec::new();
        let mut p: Vec<u32> = Vec::new();
        // Polygons as (first corner, corner count); strips and fans by kind.
        let mut polys: Vec<(usize, usize)> = Vec::new();
        match kind {
            "triangles" | "polylist" => {
                if let Some(pe) = kid(prim, "p") {
                    p = self.indices(pe, &format!("{at} <p>"))?;
                }
                if !p.len().is_multiple_of(stride) {
                    return Err(malformed(format!("{at} <p> holds {} indices, not a multiple of its {stride} inputs", p.len())));
                }
                let n_corners = p.len() / stride;
                if kind == "triangles" {
                    let count = declared.unwrap_or(n_corners / 3);
                    if count.checked_mul(3) != Some(n_corners) {
                        return Err(malformed(format!("{at} declares {count} triangles and holds {n_corners} corners")));
                    }
                    tris.reserve(count);
                    for t in 0..count {
                        tris.push([3 * t, 3 * t + 1, 3 * t + 2]);
                    }
                    return Ok((p, tris));
                }
                let vcount = match kid(prim, "vcount") {
                    Some(v) => self.indices(v, &format!("{at} <vcount>"))?,
                    None => Vec::new(),
                };
                if let Some(c) = declared {
                    if c != vcount.len() {
                        return Err(malformed(format!("{at} declares {c} polygons and its <vcount> lists {}", vcount.len())));
                    }
                }
                let total: usize = vcount.iter().map(|v| *v as usize).fold(0usize, |a, b| a.saturating_add(b));
                if total != n_corners {
                    return Err(malformed(format!("{at} <vcount> sums to {total} corners and <p> holds {n_corners}")));
                }
                let mut start = 0;
                for n in vcount {
                    polys.push((start, n as usize));
                    start += n as usize;
                }
            }
            "polygons" | "trifans" | "tristrips" => {
                if kid(prim, "ph").is_some() {
                    return Err(unsupported(format!("{at} has polygons with holes (<ph>), which this converter does not triangulate")));
                }
                let ps: Vec<&Element> = kids(prim, "p").collect();
                if let Some(c) = declared {
                    if c != ps.len() {
                        return Err(malformed(format!("{at} declares {c} and holds {} <p> elements", ps.len())));
                    }
                }
                for pe in ps {
                    let one = self.indices(pe, &format!("{at} <p>"))?;
                    if !one.len().is_multiple_of(stride) {
                        return Err(malformed(format!("{at} has a <p> of {} indices, not a multiple of its {stride} inputs", one.len())));
                    }
                    polys.push((p.len() / stride, one.len() / stride));
                    p.extend(one);
                }
            }
            _ => return Ok((p, tris)),
        }
        let position_of = |c: usize| -> Option<[f64; 3]> {
            let i = *p.get(c * stride + pos_offset)? as usize;
            if i >= pview.count {
                return None;
            }
            Some([pv[pview.at(i, 0)] as f64, pv[pview.at(i, 1)] as f64, pv[pview.at(i, 2)] as f64])
        };
        let mut degenerate = 0usize;
        let mut fanned_concave = 0usize;
        for (start, n) in polys {
            if n < 3 {
                degenerate += 1;
                continue;
            }
            match kind {
                "tristrips" => {
                    for k in 0..n - 2 {
                        let (a, b, c) = (start + k, start + k + 1, start + k + 2);
                        tris.push(if k % 2 == 0 { [a, b, c] } else { [b, a, c] });
                    }
                }
                "trifans" => {
                    for k in 1..n - 1 {
                        tris.push([start, start + k, start + k + 1]);
                    }
                }
                _ if n == 3 => tris.push([start, start + 1, start + 2]),
                _ => {
                    let pts: Option<Vec<[f64; 3]>> = (start..start + n).map(position_of).collect();
                    let Some(pts) = pts else {
                        return Err(malformed(format!("{at} has a polygon whose position index is out of range")));
                    };
                    match triangulate(&pts) {
                        Triangulation::Fan => {
                            for k in 1..n - 1 {
                                tris.push([start, start + k, start + k + 1]);
                            }
                        }
                        Triangulation::Ears(ears) => {
                            for t in ears {
                                tris.push([start + t[0], start + t[1], start + t[2]]);
                            }
                        }
                        Triangulation::TooLarge => {
                            fanned_concave += 1;
                            for k in 1..n - 1 {
                                tris.push([start, start + k, start + k + 1]);
                            }
                        }
                    }
                }
            }
            if tris.len() > MAX_SCENE_INDICES / 3 {
                return Err(unsupported(format!("{at} triangulates past the scene's index limit")));
            }
        }
        if degenerate > 0 {
            self.warn(format!("{at} has {degenerate} polygon(s) of fewer than 3 corners; they were skipped"));
        }
        if fanned_concave > 0 {
            self.warn(format!(
                "{at} has {fanned_concave} concave polygon(s) of more than {MAX_EAR_CLIP} corners; they were fan-triangulated"
            ));
        }
        Ok((p, tris))
    }

    // ── materials ───────────────────────────────────────────────────────────

    fn binding(&mut self, inst: &'a Element) -> Result<Binding<'a>, MeshError> {
        let mut entries = Vec::new();
        for bm in kids(inst, "bind_material") {
            for tc in kids(bm, "technique_common") {
                for im in kids(tc, "instance_material") {
                    let symbol = im.attr("symbol").ok_or_else(|| malformed("an <instance_material> has no symbol"))?;
                    let target = im.attr("target").ok_or_else(|| malformed(format!("instance_material {} has no target", q(symbol))))?;
                    let mat = self.doc.resolve(target, "material", &format!("instance_material {}", q(symbol)))?;
                    let mut inputs = Vec::new();
                    for b in kids(im, "bind_vertex_input") {
                        let (Some(sem), Some(isem)) = (b.attr("semantic"), b.attr("input_semantic")) else { continue };
                        if isem != "TEXCOORD" {
                            continue;
                        }
                        let set = match b.attr("input_set") {
                            None => 0,
                            Some(s) => s.trim().parse::<u32>().map_err(|_| malformed(format!("bind_vertex_input input_set {} is not a number", q(s))))?,
                        };
                        inputs.push((sem.to_string(), set));
                    }
                    entries.push((symbol, mat, inputs));
                }
            }
        }
        let mut signature = String::new();
        for (symbol, mat, inputs) in &entries {
            let _ = write!(signature, "{symbol}\u{1}{:x}", mat_ptr(mat));
            for (s, set) in inputs {
                let _ = write!(signature, "\u{1}{s}={set}");
            }
            signature.push('\u{2}');
        }
        Ok(Binding { entries, signature })
    }

    fn effect(&mut self, effect: &'a Element) -> Result<Rc<EffectData>, MeshError> {
        if let Some(e) = self.effect_cache.get(&mat_ptr(effect)) {
            return Ok(e.clone());
        }
        let eid = effect.attr("id").unwrap_or("");
        let mut data = EffectData {
            shading: Shading::Lambert,
            emission: None,
            diffuse: None,
            transparent: None,
            transparency: None,
            shininess: None,
            double_sided: double_sided_extra(effect),
        };
        let Some(profile) = kid(effect, "profile_COMMON") else {
            self.warn(format!("effect {} has no profile_COMMON (only shader profiles); it reads as a white material", q(eid)));
            let rc = Rc::new(data);
            self.effect_cache.insert(mat_ptr(effect), rc.clone());
            return Ok(rc);
        };
        data.double_sided |= double_sided_extra(profile);
        // newparams may sit on the effect or on the profile.
        let mut params: HashMap<&'a str, &'a Element> = HashMap::new();
        for np in kids(effect, "newparam").chain(kids(profile, "newparam")) {
            if let Some(sid) = np.attr("sid") {
                params.entry(sid).or_insert(np);
            }
        }
        let technique = kid(profile, "technique").ok_or_else(|| malformed(format!("effect {} profile_COMMON has no technique", q(eid))))?;
        data.double_sided |= double_sided_extra(technique);
        let Some(shader) = technique.children.iter().find(|c| matches!(c.name.as_str(), "constant" | "lambert" | "phong" | "blinn")) else {
            self.warn(format!("effect {} has no constant/lambert/phong/blinn shader; it reads as white", q(eid)));
            let rc = Rc::new(data);
            self.effect_cache.insert(mat_ptr(effect), rc.clone());
            return Ok(rc);
        };
        data.shading = match shader.name.as_str() {
            "constant" => Shading::Constant,
            "lambert" => Shading::Lambert,
            _ => Shading::Phong,
        };
        for c in &shader.children {
            let at = format!("effect {} <{}>", q(eid), clip(&c.name, 30));
            match c.name.as_str() {
                "emission" => data.emission = self.color_or_texture(c, &params, &at)?,
                "diffuse" => data.diffuse = self.color_or_texture(c, &params, &at)?,
                "transparent" => {
                    let mode = c.attr("opaque").unwrap_or("A_ONE").to_string();
                    if let Some(v) = self.color_or_texture(c, &params, &at)? {
                        data.transparent = Some((mode, v));
                    }
                }
                "transparency" => data.transparency = self.float_param(c, &params, &at)?,
                "shininess" => data.shininess = self.float_param(c, &params, &at)?,
                "bump" => self.warn_once(
                    "bump",
                    format!("{at}: bump maps are not carried (Collada does not say whether one is a height or a normal map)"),
                ),
                _ => {}
            }
        }
        for e in kids(technique, "extra").chain(kids(profile, "extra")) {
            if !e.descendants_named("bump").is_empty() {
                self.warn_once(
                    "bump",
                    format!("effect {}: bump maps are not carried (Collada does not say whether one is a height or a normal map)", q(eid)),
                );
            }
        }
        let rc = Rc::new(data);
        self.effect_cache.insert(mat_ptr(effect), rc.clone());
        Ok(rc)
    }

    fn color_or_texture(
        &mut self,
        e: &'a Element,
        params: &HashMap<&'a str, &'a Element>,
        at: &str,
    ) -> Result<Option<TexSource>, MeshError> {
        for c in &e.children {
            match c.name.as_str() {
                "color" => return Ok(Some(TexSource::Color(color4(&c.text, at)?))),
                "param" => {
                    let Some(np) = c.attr("ref").and_then(|r| params.get(r)) else {
                        self.warn(format!("{at} names a <param> that is not declared; it was ignored"));
                        return Ok(None);
                    };
                    if let Some(f4) = kid(np, "float4").or_else(|| kid(np, "float3")) {
                        return Ok(Some(TexSource::Color(color4(&f4.text, at)?)));
                    }
                    self.warn(format!("{at} names a <param> that is not a colour; it was ignored"));
                    return Ok(None);
                }
                "texture" => return self.texture(c, params, at),
                _ => {}
            }
        }
        Ok(None)
    }

    fn float_param(&mut self, e: &'a Element, params: &HashMap<&'a str, &'a Element>, at: &str) -> Result<Option<f32>, MeshError> {
        for c in &e.children {
            let text = match c.name.as_str() {
                "float" => &c.text,
                "param" => match c.attr("ref").and_then(|r| params.get(r)).and_then(|np| kid(np, "float")) {
                    Some(f) => &f.text,
                    None => continue,
                },
                _ => continue,
            };
            let v = text
                .trim()
                .parse::<f32>()
                .ok()
                .filter(|v| v.is_finite())
                .ok_or_else(|| malformed(format!("{at} holds {}, not a number", q(text))))?;
            return Ok(Some(v));
        }
        Ok(None)
    }

    /// `<texture texture="sampler sid" texcoord="...">` → the scene texture,
    /// through the sampler2D → surface → image chain.
    fn texture(&mut self, tex: &'a Element, params: &HashMap<&'a str, &'a Element>, at: &str) -> Result<Option<TexSource>, MeshError> {
        let name = tex.attr("texture").unwrap_or("");
        let texcoord = tex.attr("texcoord").unwrap_or("").to_string();
        let mut sampler = unspecified_sampler();
        let image_el: Option<&'a Element> = match params.get(name).and_then(|np| kid(np, "sampler2D")) {
            Some(s2d) => {
                sampler = sampler_of(s2d, at, &mut self.warnings);
                // 1.5 names the image directly; 1.4 goes through a surface.
                if let Some(ii) = kid(s2d, "instance_image") {
                    ii.attr("url").and_then(|u| self.doc.get(u))
                } else {
                    let surface_sid = kid(s2d, "source").map(|s| s.text.trim()).unwrap_or("");
                    let init = params
                        .get(surface_sid)
                        .and_then(|np| kid(np, "surface"))
                        .and_then(|s| kid(s, "init_from"))
                        .map(|i| i.text.trim());
                    init.and_then(|id| self.doc.by_id(id))
                }
            }
            None => {
                // Several exporters name the image itself here; three.js
                // accepts that too (its "#12530" fallback).
                let direct = self.doc.by_id(name).filter(|e| e.name == "image");
                if direct.is_some() {
                    self.warn_once(
                        "direct texture",
                        format!("{at} names image {} directly instead of a sampler; it was used as one", q(name)),
                    );
                }
                direct
            }
        };
        if let Some(extra_wrap) = maya_wrap(tex) {
            if params.get(name).and_then(|np| kid(np, "sampler2D")).is_none_or(|s| kid(s, "wrap_s").is_none()) {
                sampler.wrap_s = extra_wrap.0;
                sampler.wrap_t = extra_wrap.1;
            }
        }
        let Some(image_el) = image_el.filter(|e| e.name == "image") else {
            self.warn(format!("{at} names texture {} whose image cannot be resolved; it was dropped", q(name)));
            return Ok(None);
        };
        let Some(image) = self.image(image_el)? else { return Ok(None) };
        let key = (image, format!("{sampler:?}"));
        let texture = match self.texture_cache.get(&key) {
            Some(&t) => t,
            None => {
                self.scene.textures.push(Texture { name: self.scene.images[image].name.clone(), image, sampler });
                let t = self.scene.textures.len() - 1;
                self.texture_cache.insert(key, t);
                t
            }
        };
        Ok(Some(TexSource::Texture { texture, image, texcoord }))
    }

    fn image(&mut self, img: &'a Element) -> Result<Option<usize>, MeshError> {
        if let Some(i) = self.image_cache.get(&mat_ptr(img)) {
            return Ok(*i);
        }
        let iid = img.attr("id").unwrap_or("");
        let bytes: Option<Vec<u8>> = if let Some(data) = kid(img, "data") {
            Some(decode_hex(&data.text).ok_or_else(|| malformed(format!("image {} <data> is not hexadecimal", q(iid))))?)
        } else if let Some(init) = kid(img, "init_from") {
            // 1.5 wraps the URI in <ref> (or embeds <hex>).
            let uri = kid(init, "ref").map(|r| r.text.trim()).unwrap_or(init.text.trim()).to_string();
            if let Some(hex) = kid(init, "hex") {
                Some(decode_hex(&hex.text).ok_or_else(|| malformed(format!("image {} <hex> is not hexadecimal", q(iid))))?)
            } else {
                self.load_sidecar(&uri, &format!("image {}", q(iid)))
            }
        } else {
            self.warn(format!("image {} has neither <init_from> nor <data>; its textures were dropped", q(iid)));
            None
        };
        let out = match bytes {
            None => None,
            Some(data) => match ImageFormat::sniff(&data) {
                Some(format) => {
                    self.scene.images.push(Image {
                        name: img.attr("name").filter(|s| !s.is_empty()).map(str::to_string),
                        format,
                        data,
                    });
                    Some(self.scene.images.len() - 1)
                }
                None => {
                    self.warn(format!("image {} is not PNG, JPEG, WebP or KTX2; its textures were dropped", q(iid)));
                    None
                }
            },
        };
        self.image_cache.insert(mat_ptr(img), out);
        Ok(out)
    }

    /// Bytes of a file the document refers to, from the caller's sidecars.
    ///
    /// Relative references resolve inside the package. Absolute paths and
    /// `file:` URIs — what DCC tools write when they export from a project
    /// directory — cannot, but the same file shipped beside the document under
    /// its own name is unambiguous, so that is tried before giving up.
    fn load_sidecar(&mut self, uri: &str, what: &str) -> Option<Vec<u8>> {
        if uri.is_empty() {
            self.warn(format!("{what} has an empty reference; its textures were dropped"));
            return None;
        }
        if uri.len() >= 5 && uri[..5].eq_ignore_ascii_case("data:") {
            self.warn(format!("{what} is a data: URI, which this reader does not decode; its textures were dropped"));
            return None;
        }
        if let Some(bytes) = package::normalize_relative(uri).and_then(|p| self.sidecars.get(&p)) {
            return Some(bytes);
        }
        let decoded = package::percent_decode(uri);
        let base = decoded.rsplit(['/', '\\']).next().unwrap_or("");
        if base != decoded.as_str() || package::normalize_relative(uri).is_none() {
            if let Some(bytes) = package::normalize_relative(base).and_then(|p| self.sidecars.get(&p)) {
                self.warn(format!("{what} refers to {}; the file of that name beside the document was used", q(uri)));
                return Some(bytes);
            }
        }
        self.warn(format!("{what} refers to {}, which was not supplied; its textures were dropped", q(uri)));
        None
    }

    /// The scene material for a Collada material bound with `inputs`, as
    /// drawn by a primitive whose texture coordinate sets are `prim_sets`.
    fn material(
        &mut self,
        mat: &'a Element,
        inputs: &[(String, u32)],
        prim_sets: &[u32],
        double_sided: bool,
    ) -> Result<Option<usize>, MeshError> {
        let mid = mat.attr("id").unwrap_or("");
        let effect_url = kid(mat, "instance_effect").and_then(|i| i.attr("url"));
        let Some(effect_url) = effect_url else {
            self.warn(format!("material {} has no instance_effect; it reads as white", q(mid)));
            return Ok(None);
        };
        let effect = self.doc.resolve(effect_url, "effect", &format!("material {}", q(mid)))?;
        let fx = self.effect(effect)?;
        // Which scene UV set a texcoord name means for this primitive.
        let uv_index = |texcoord: &str| -> Option<u32> {
            let set = inputs.iter().find(|(s, _)| s == texcoord).map(|(_, set)| *set).or(prim_sets.first().copied())?;
            prim_sets.iter().position(|s| *s == set).map(|i| i as u32)
        };
        let mut key = format!("{:x}\u{1}{}", mat_ptr(mat), double_sided);
        for src in [&fx.diffuse, &fx.emission].into_iter().flatten() {
            if let TexSource::Texture { texcoord, .. } = src {
                let _ = write!(key, "\u{1}{:?}", uv_index(texcoord));
            }
        }
        if let Some(i) = self.material_cache.get(&key) {
            return Ok(*i);
        }
        let mut m = Material {
            name: mat.attr("name").filter(|s| !s.is_empty()).map(str::to_string),
            metallic: 0.0,
            roughness: 1.0,
            double_sided: fx.double_sided || double_sided,
            ..Default::default()
        };
        let texture_ref = |src: &TexSource, what: &str, warnings: &mut Vec<String>| -> Option<TextureRef> {
            let TexSource::Texture { texture, texcoord, .. } = src else { return None };
            match uv_index(texcoord) {
                Some(uv_set) => Some(TextureRef { texture: *texture, uv_set, transform: None }),
                None => {
                    warnings.push(clip(
                        &format!("material {}: the {what} texture's coordinates ({}) are not on the primitive; it was dropped", q(mid), q(texcoord)),
                        MAX_DETAIL,
                    ));
                    None
                }
            }
        };
        let mut diffuse_image = None;
        match &fx.diffuse {
            Some(TexSource::Color(c)) => {
                m.base_color = [srgb_decode(c[0]), srgb_decode(c[1]), srgb_decode(c[2]), 1.0];
            }
            Some(t @ TexSource::Texture { image, .. }) => {
                m.base_color_texture = texture_ref(t, "diffuse", &mut self.warnings);
                diffuse_image = Some(*image);
            }
            None => {}
        }
        match &fx.emission {
            Some(TexSource::Color(c)) => m.emissive = [srgb_decode(c[0]), srgb_decode(c[1]), srgb_decode(c[2])],
            Some(t @ TexSource::Texture { .. }) => {
                m.emissive_texture = texture_ref(t, "emission", &mut self.warnings);
                if m.emissive_texture.is_some() {
                    m.emissive = [1.0; 3];
                }
            }
            None => {}
        }
        match fx.shading {
            Shading::Constant => {
                // A constant shader's colour IS its emission (spec §8); the
                // scene says that with an unlit material.
                m.unlit = true;
                if fx.diffuse.is_none() {
                    if let Some(TexSource::Color(c)) = &fx.emission {
                        m.base_color = [srgb_decode(c[0]), srgb_decode(c[1]), srgb_decode(c[2]), 1.0];
                        m.emissive = [0.0; 3];
                    }
                }
            }
            Shading::Lambert => m.roughness = 1.0,
            Shading::Phong => m.roughness = roughness_from_shininess(fx.shininess.unwrap_or(0.0)),
        }
        // Transparency (spec §8 "Determining Transparency").
        let factor = fx.transparency.unwrap_or(1.0);
        let (opacity, from_texture) = match &fx.transparent {
            None => (factor, false),
            Some((mode, TexSource::Color(c))) => {
                let lum = 0.212_671 * c[0] + 0.715_160 * c[1] + 0.072_169 * c[2];
                let o = match mode.as_str() {
                    "RGB_ZERO" => 1.0 - lum * factor,
                    "A_ZERO" => 1.0 - c[3] * factor,
                    "RGB_ONE" => lum * factor,
                    _ => c[3] * factor,
                };
                (o, false)
            }
            Some((mode, TexSource::Texture { image, .. })) => {
                if mode == "A_ONE" && Some(*image) == diffuse_image {
                    (factor, true)
                } else {
                    self.warn(format!(
                        "material {}: opacity from a separate texture ({mode}) is not carried; it was ignored",
                        q(mid)
                    ));
                    (1.0, false)
                }
            }
        };
        if from_texture || opacity < 1.0 - 1e-6 {
            m.alpha_mode = AlphaMode::Blend;
            m.base_color[3] = opacity.clamp(0.0, 1.0);
            if opacity <= 0.0 && !from_texture {
                self.warn(format!(
                    "material {} is fully transparent by the spec's transparency rules; some exporters invert \
                     <transparency>, so check its source",
                    q(mid)
                ));
            }
        }
        self.scene.materials.push(m);
        let i = Some(self.scene.materials.len() - 1);
        self.material_cache.insert(key, i);
        Ok(i)
    }

    // ── controllers ─────────────────────────────────────────────────────────

    fn controller(&mut self, ctrl: &'a Element) -> Result<Rc<Controller<'a>>, MeshError> {
        if let Some(c) = self.controller_cache.get(&mat_ptr(ctrl)) {
            return Ok(c.clone());
        }
        let cid = ctrl.attr("id").unwrap_or("");
        let out = if let Some(skin) = kid(ctrl, "skin") {
            Controller::Skin(Rc::new(self.skin(ctrl, skin)?))
        } else if let Some(morph) = kid(ctrl, "morph") {
            Controller::Morph(Rc::new(self.morph(ctrl, morph)?))
        } else {
            return Err(unsupported(format!("controller {} is neither a skin nor a morph", q(cid))));
        };
        let rc = Rc::new(out);
        self.controller_cache.insert(mat_ptr(ctrl), rc.clone());
        Ok(rc)
    }

    fn skin(&mut self, ctrl: &'a Element, skin: &'a Element) -> Result<SkinData<'a>, MeshError> {
        let cid = ctrl.attr("id").unwrap_or("");
        let at = format!("skin controller {}", q(cid));
        let src_url = skin.attr("source").ok_or_else(|| malformed(format!("{at} has no source")))?;
        let target = self.doc.get(src_url).ok_or_else(|| malformed(format!("{at} source {} is not in the document", q(src_url))))?;
        let (geometry, morph) = match target.name.as_str() {
            "geometry" => (target, None),
            // Only a morph may sit under a skin. Checked on the element, before
            // anything is resolved: resolving a skin's source skin would
            // recurse as deep as a hostile chain goes (or forever, for a skin
            // that names itself).
            "controller" if kid(target, "morph").is_some() => match &*self.controller(target)? {
                Controller::Morph(m) => (m.base, Some(m.clone())),
                Controller::Skin(_) => return Err(unsupported(format!("{at} skins another skin"))),
            },
            "controller" => return Err(unsupported(format!("{at} skins another skin; only a morph may sit under a skin"))),
            other => return Err(malformed(format!("{at} source is a <{}>", clip(other, 30)))),
        };
        let bind_shape = match kid(skin, "bind_shape_matrix") {
            None => math::IDENTITY,
            Some(b) => {
                let v = parse_f64_list(&b.text, &format!("{at} bind_shape_matrix"))?;
                matrix_from_row_major(&v).ok_or_else(|| malformed(format!("{at} bind_shape_matrix has {} values", v.len())))?
            }
        };
        let joints_el = kid(skin, "joints").ok_or_else(|| malformed(format!("{at} has no <joints>")))?;
        let mut joint_src = None;
        let mut ibm_src = None;
        for i in kids(joints_el, "input") {
            let url = i.attr("source").unwrap_or("");
            match i.attr("semantic") {
                Some("JOINT") => joint_src = Some(self.doc.resolve(url, "source", &format!("{at} JOINT"))?),
                Some("INV_BIND_MATRIX") => ibm_src = Some(self.doc.resolve(url, "source", &format!("{at} INV_BIND_MATRIX"))?),
                _ => {}
            }
        }
        let joint_src = joint_src.ok_or_else(|| malformed(format!("{at} <joints> has no JOINT input")))?;
        let (names, nview) = self.stream_names(joint_src, "JOINT")?;
        let joints_are_ids = nview.array.name == "IDREF_array";
        let joints: Vec<String> = (0..nview.count).map(|i| names[nview.at(i, 0)].clone()).collect();
        if joints.is_empty() || joints.len() > MAX_JOINTS {
            return Err(malformed(format!("{at} has {} joints", joints.len())));
        }
        let ibms: Vec<Mat4> = match ibm_src {
            None => vec![bind_shape; joints.len()],
            Some(src) => {
                let (vals, v) = self.stream_f64(src, 16, "INV_BIND_MATRIX")?;
                if v.count != joints.len() {
                    return Err(malformed(format!("{at} has {} inverse bind matrices for {} joints", v.count, joints.len())));
                }
                (0..v.count)
                    .map(|i| {
                        let row: Vec<f64> = (0..16).map(|c| vals[v.at(i, c)]).collect();
                        let ibm = matrix_from_row_major(&row).unwrap_or(math::IDENTITY);
                        // The scene's skinning has no bind-shape matrix: it
                        // is folded into each joint's inverse bind.
                        math::mat_mul(&ibm, &bind_shape)
                    })
                    .collect()
            }
        };
        let vw = kid(skin, "vertex_weights").ok_or_else(|| malformed(format!("{at} has no <vertex_weights>")))?;
        let count = attr_count(vw, "count", &at)?.ok_or_else(|| malformed(format!("{at} vertex_weights has no count")))?;
        let mut w_stride = 0usize;
        let mut joint_off = None;
        let mut weight = None;
        for i in kids(vw, "input") {
            let off = attr_count(i, "offset", &at)?.ok_or_else(|| malformed(format!("{at} vertex_weights input has no offset")))?;
            if off > MAX_INPUT_OFFSET {
                return Err(malformed(format!("{at} vertex_weights input offset {off}")));
            }
            w_stride = w_stride.max(off + 1);
            match i.attr("semantic") {
                Some("JOINT") => joint_off = Some(off),
                Some("WEIGHT") => {
                    let url = i.attr("source").unwrap_or("");
                    weight = Some((off, self.doc.resolve(url, "source", &format!("{at} WEIGHT"))?));
                }
                _ => {}
            }
        }
        let joint_off = joint_off.ok_or_else(|| malformed(format!("{at} vertex_weights has no JOINT input")))?;
        let (weight_off, weight_src) = weight.ok_or_else(|| malformed(format!("{at} vertex_weights has no WEIGHT input")))?;
        let (wvals, wview) = self.stream_f32(weight_src, 1, "WEIGHT")?;
        let vcount = match kid(vw, "vcount") {
            Some(v) => self.indices(v, &format!("{at} <vcount>"))?,
            None => Vec::new(),
        };
        if vcount.len() != count {
            return Err(malformed(format!("{at} declares {count} weighted vertices and its <vcount> lists {}", vcount.len())));
        }
        let v = match kid(vw, "v") {
            Some(v) => self.signed(v, &format!("{at} <v>"))?,
            None => Vec::new(),
        };
        let total: usize = vcount.iter().map(|c| *c as usize).fold(0usize, |a, b| a.saturating_add(b));
        if total.checked_mul(w_stride) != Some(v.len()) {
            return Err(malformed(format!("{at} <vcount> sums to {total} influences and <v> holds {} values", v.len())));
        }
        let mut starts = Vec::with_capacity(count + 1);
        let mut pairs = Vec::with_capacity(total);
        let mut cursor = 0usize;
        for n in &vcount {
            starts.push(pairs.len());
            for _ in 0..*n {
                let j = v[cursor + joint_off];
                let wi = v[cursor + weight_off];
                cursor += w_stride;
                if j < -1 || j >= joints.len() as i64 {
                    return Err(malformed(format!("{at} names joint {j} of {}", joints.len())));
                }
                if wi < 0 || wi as usize >= wview.count {
                    return Err(malformed(format!("{at} names weight {wi} of {}", wview.count)));
                }
                pairs.push((j, wvals[wview.at(wi as usize, 0)]));
            }
        }
        starts.push(pairs.len());
        Ok(SkinData {
            name: ctrl.attr("name").filter(|s| !s.is_empty()).map(str::to_string),
            geometry,
            morph,
            joints,
            joints_are_ids,
            ibms,
            bind_shape,
            starts,
            pairs,
        })
    }

    fn morph(&mut self, ctrl: &'a Element, morph: &'a Element) -> Result<MorphData<'a>, MeshError> {
        let cid = ctrl.attr("id").unwrap_or("");
        let at = format!("morph controller {}", q(cid));
        let url = morph.attr("source").ok_or_else(|| malformed(format!("{at} has no source")))?;
        let base = self.doc.get(url).ok_or_else(|| malformed(format!("{at} source {} is not in the document", q(url))))?;
        if base.name != "geometry" {
            return Err(unsupported(format!("{at} morphs a <{}>; only geometry bases are supported", clip(&base.name, 30))));
        }
        let relative = match morph.attr("method").unwrap_or("NORMALIZED") {
            "NORMALIZED" => false,
            "RELATIVE" => true,
            other => return Err(malformed(format!("{at} method {} is neither NORMALIZED nor RELATIVE", q(other)))),
        };
        let targets_el = kid(morph, "targets").ok_or_else(|| malformed(format!("{at} has no <targets>")))?;
        let mut target_src = None;
        let mut weight_src = None;
        for i in kids(targets_el, "input") {
            let url = i.attr("source").unwrap_or("");
            match i.attr("semantic") {
                Some("MORPH_TARGET") => target_src = Some(self.doc.resolve(url, "source", &format!("{at} MORPH_TARGET"))?),
                Some("MORPH_WEIGHT") => weight_src = Some(self.doc.resolve(url, "source", &format!("{at} MORPH_WEIGHT"))?),
                _ => {}
            }
        }
        let target_src = target_src.ok_or_else(|| malformed(format!("{at} has no MORPH_TARGET input")))?;
        let weight_src = weight_src.ok_or_else(|| malformed(format!("{at} has no MORPH_WEIGHT input")))?;
        let (ids, tview) = self.stream_names(target_src, "MORPH_TARGET")?;
        let (wvals, wview) = self.stream_f32(weight_src, 1, "MORPH_WEIGHT")?;
        if tview.count != wview.count {
            return Err(malformed(format!("{at} has {} targets and {} weights", tview.count, wview.count)));
        }
        if tview.count > MAX_MORPH_TARGETS {
            return Err(unsupported(format!("{at} has {} morph targets, over the {MAX_MORPH_TARGETS} limit", tview.count)));
        }
        let mut targets = Vec::with_capacity(tview.count);
        let mut names = Vec::with_capacity(tview.count);
        for i in 0..tview.count {
            let id = &ids[tview.at(i, 0)];
            let g = self
                .doc
                .by_id(id)
                .filter(|e| e.name == "geometry")
                .ok_or_else(|| malformed(format!("{at} target {} is not a geometry of the document", q(id))))?;
            names.push(g.attr("name").unwrap_or(id).to_string());
            targets.push(g);
        }
        let weights = (0..wview.count).map(|i| wvals[wview.at(i, 0)]).collect();
        Ok(MorphData { controller: mat_ptr(ctrl), base, relative, targets, names, weights })
    }

    // ── visual scene ────────────────────────────────────────────────────────

    fn read_asset(&mut self) -> Result<(), MeshError> {
        let Some(asset) = kid(self.doc.root, "asset") else { return Ok(()) };
        let meter = match kid(asset, "unit").and_then(|u| u.attr("meter")) {
            None => 1.0,
            Some(m) => m
                .trim()
                .parse::<f64>()
                .ok()
                .filter(|m| m.is_finite() && *m > 0.0)
                .ok_or_else(|| malformed(format!("<unit meter={}> is not a positive length", q(m))))?,
        };
        let up = kid(asset, "up_axis").map(|u| u.text.trim().to_string()).unwrap_or_else(|| "Y_UP".into());
        let rotation = match up.as_str() {
            "Y_UP" => None,
            // Z up: right +x, up +z, in −y (spec 1.4.1 §5 asset): a −90° turn about X.
            "Z_UP" => Some(math::quat_to_mat(math::quat_from_axis_angle([1.0, 0.0, 0.0], -std::f64::consts::FRAC_PI_2))),
            // X up: right −y, up +x, in +z: a +90° turn about Z.
            "X_UP" => Some(math::quat_to_mat(math::quat_from_axis_angle([0.0, 0.0, 1.0], std::f64::consts::FRAC_PI_2))),
            other => return Err(malformed(format!("<up_axis> is {}, not X_UP, Y_UP or Z_UP", q(other)))),
        };
        if meter != 1.0 || rotation.is_some() {
            let mut c = math::scaling([meter; 3]);
            if let Some(r) = rotation {
                c = math::mat_mul(&r, &c);
            }
            // Rotations of exactly ±90° in f64 leave 6e-17 where zero belongs;
            // snapping keeps axis-aligned data axis-aligned.
            for v in c.iter_mut() {
                if v.abs() < 1e-15 {
                    *v = 0.0;
                }
            }
            self.correction = Some(c);
        }
        Ok(())
    }

    fn read_nodes(&mut self) -> Result<(), MeshError> {
        let scene_el = kid(self.doc.root, "scene");
        let vs = match scene_el.and_then(|s| kid(s, "instance_visual_scene")).and_then(|i| i.attr("url")) {
            Some(url) => Some(self.doc.resolve(url, "visual_scene", "<scene>")?),
            None => {
                let first = self.doc.library("library_visual_scenes", "visual_scene").into_iter().next();
                if first.is_some() {
                    self.warn("the document has no <scene>; its first visual scene was used".into());
                }
                first
            }
        };
        let Some(vs) = vs else {
            // A library of geometry with no scene: take every mesh at identity,
            // as the glTF reader does for a scene-less document.
            self.warn("the document has no visual scene; every geometry was placed at the origin unmaterialled".into());
            for g in self.doc.library("library_geometries", "geometry") {
                let binding = Rc::new(Binding { entries: Vec::new(), signature: String::new() });
                let ir = self.push_node(None, None, Rc::new(Vec::new()), None)?;
                self.info[ir].parts.push(Part { source: PartSource::Geometry(g), binding, skeletons: Vec::new() });
            }
            return Ok(());
        };
        self.scene.name = vs.attr("name").filter(|s| !s.is_empty()).map(str::to_string);

        enum Work<'a> {
            Enter { el: &'a Element, parent: Option<usize>, depth: usize },
            Leave(usize),
        }
        // Element addresses on the current path: an instance_node naming one
        // of them would expand forever.
        let mut active: HashSet<usize> = HashSet::new();
        let mut stack: Vec<Work<'a>> =
            kids(vs, "node").collect::<Vec<_>>().into_iter().rev().map(|el| Work::Enter { el, parent: None, depth: 1 }).collect();
        let mut cameras = 0usize;
        let mut lights = 0usize;
        while let Some(w) = stack.pop() {
            let (el, parent, depth) = match w {
                Work::Leave(p) => {
                    active.remove(&p);
                    continue;
                }
                Work::Enter { el, parent, depth } => (el, parent, depth),
            };
            if depth > MAX_NODE_DEPTH {
                return Err(unsupported(format!("nodes nest deeper than {MAX_NODE_DEPTH}")));
            }
            let ops = self.node_ops(el)?;
            let ir = self.push_node(Some(el), parent, ops, el.attr("name"))?;
            for c in &el.children {
                match c.name.as_str() {
                    "instance_geometry" => {
                        let url = c.attr("url").unwrap_or("");
                        let g = self.doc.resolve(url, "geometry", &format!("node {} instance_geometry", q(el.attr("id").unwrap_or(""))))?;
                        let binding = Rc::new(self.binding(c)?);
                        self.info[ir].parts.push(Part { source: PartSource::Geometry(g), binding, skeletons: Vec::new() });
                    }
                    "instance_controller" => {
                        let url = c.attr("url").unwrap_or("");
                        let ctrl = self.doc.resolve(url, "controller", &format!("node {} instance_controller", q(el.attr("id").unwrap_or(""))))?;
                        let binding = Rc::new(self.binding(c)?);
                        let skeletons = kids(c, "skeleton").map(|s| s.text.trim().to_string()).collect();
                        self.info[ir].parts.push(Part { source: PartSource::Controller(ctrl), binding, skeletons });
                    }
                    "instance_camera" => cameras += 1,
                    "instance_light" => lights += 1,
                    _ => {}
                }
            }
            active.insert(mat_ptr(el));
            stack.push(Work::Leave(mat_ptr(el)));
            for c in el.children.iter().rev() {
                match c.name.as_str() {
                    "node" => stack.push(Work::Enter { el: c, parent: Some(ir), depth: depth + 1 }),
                    "instance_node" => {
                        let url = c.attr("url").unwrap_or("");
                        let target = self.doc.resolve(url, "node", &format!("node {} instance_node", q(el.attr("id").unwrap_or(""))))?;
                        if active.contains(&mat_ptr(target)) {
                            return Err(malformed(format!("instance_node {} instances its own ancestor; the node graph is a cycle", q(url))));
                        }
                        stack.push(Work::Enter { el: target, parent: Some(ir), depth: depth + 1 });
                    }
                    _ => {}
                }
            }
        }
        if cameras + lights > 0 {
            self.warn(format!("{cameras} camera(s) and {lights} light(s) are not carried"));
        }
        Ok(())
    }

    /// `el`'s transform stack, parsed and charged on first use and shared
    /// after: an `instance_node` re-enters its target once per copy, and
    /// re-parsing per copy made memory grow as copies x transforms.
    fn node_ops(&mut self, el: &'a Element) -> Result<Rc<Vec<Op>>, MeshError> {
        if let Some(ops) = self.ops_by_element.get(&mat_ptr(el)) {
            return Ok(ops.clone());
        }
        let ops = parse_ops(el)?;
        self.charge(ops.iter().map(|o| o.values.len()).sum(), "node transforms")?;
        let ops = Rc::new(ops);
        self.ops_by_element.insert(mat_ptr(el), ops.clone());
        Ok(ops)
    }

    fn push_node(&mut self, el: Option<&'a Element>, parent: Option<usize>, ops: Rc<Vec<Op>>, name: Option<&'a str>) -> Result<usize, MeshError> {
        if self.scene.nodes.len() >= MAX_NODES {
            return Err(unsupported(format!("the scene has more than {MAX_NODES} nodes (counting instance_node copies)")));
        }
        let m = compose_ops(&ops)?;
        let node = Node {
            name: name.filter(|s| !s.is_empty()).map(str::to_string),
            transform: Transform::from_matrix(m),
            ..Default::default()
        };
        self.scene.nodes.push(node);
        let ir = self.scene.nodes.len() - 1;
        match parent {
            Some(p) => self.scene.nodes[p].children.push(ir),
            None => self.scene.roots.push(ir),
        }
        self.info.push(NodeInfo {
            ops,
            parts: Vec::new(),
            sid: el.and_then(|e| e.attr("sid")),
            id: el.and_then(|e| e.attr("id")),
            name: el.and_then(|e| e.attr("name")),
        });
        if let Some(e) = el {
            self.nodes_by_element.entry(mat_ptr(e)).or_default().push(ir);
        }
        Ok(ir)
    }

    /// Turn every node's instances into scene meshes and skins.
    fn assemble(&mut self) -> Result<(), MeshError> {
        for ir in 0..self.info.len() {
            let parts = std::mem::take(&mut self.info[ir].parts);
            if !parts.is_empty() {
                self.assemble_node(ir, parts)?;
            }
        }
        Ok(())
    }

    fn assemble_node(&mut self, ir: usize, parts: Vec<Part<'a>>) -> Result<(), MeshError> {
        struct Resolved<'a> {
            key: String,
            geom: Rc<GeomData>,
            binding: Rc<Binding<'a>>,
            skin: Option<(Rc<SkinData<'a>>, usize, Option<usize>)>,
            morph: Option<(usize, Rc<MorphData<'a>>)>,
        }
        let mut resolved: Vec<Resolved<'a>> = Vec::new();
        for part in parts {
            let (elem, geom, skin, morph) = match part.source {
                PartSource::Geometry(g) => (g, self.geometry(g)?, None, None),
                PartSource::Controller(c) => match &*self.controller(c)? {
                    Controller::Skin(sd) => {
                        let geom = self.geometry(sd.geometry)?;
                        let (skin_index, bind_shape_joint) = self.skin_for(c, sd, &part.skeletons, ir, &geom)?;
                        let morph = sd.morph.clone().map(|m| (m.controller, m));
                        (c, geom, Some((sd.clone(), skin_index, bind_shape_joint)), morph)
                    }
                    Controller::Morph(md) => (c, self.geometry(md.base)?, None, Some((md.controller, md.clone()))),
                },
            };
            if geom.prims.is_empty() {
                continue;
            }
            let key = format!(
                "{:x}\u{1}{}\u{1}{:?}",
                mat_ptr(elem),
                part.binding.signature,
                skin.as_ref().map(|(_, s, j)| (*s, *j))
            );
            resolved.push(Resolved { key, geom, binding: part.binding, skin, morph });
        }
        // Parts that share a skin (or have none) and carry no morph merge into
        // one mesh; anything else needs a node of its own.
        let mut groups: Vec<(Option<usize>, Option<usize>, Vec<Resolved<'a>>)> = Vec::new();
        let mut by_skin: HashMap<Option<usize>, usize> = HashMap::new();
        for r in resolved {
            let skin = r.skin.as_ref().map(|(_, s, _)| *s);
            let morph = r.morph.as_ref().map(|(c, _)| *c);
            match (morph, by_skin.get(&skin)) {
                (None, Some(&g)) => groups[g].2.push(r),
                _ => {
                    if morph.is_none() {
                        by_skin.insert(skin, groups.len());
                    }
                    groups.push((skin, morph, vec![r]));
                }
            }
        }
        for (gi, (skin, morph, members)) in groups.into_iter().enumerate() {
            let key = members.iter().map(|m| m.key.as_str()).collect::<Vec<_>>().join("\u{3}");
            let mesh = match self.mesh_cache.get(&key) {
                Some(&m) => m,
                None => {
                    let m = self.build_mesh(&members.iter().map(|r| (r.geom.clone(), r.binding.clone(), r.skin.clone(), r.morph.clone())).collect::<Vec<_>>())?;
                    self.mesh_cache.insert(key, m);
                    m
                }
            };
            let target = if gi == 0 {
                ir
            } else {
                let name = self.scene.nodes[ir].name.as_ref().map(|n| format!("{n}_part{gi}"));
                let child = self.push_node(None, Some(ir), Rc::new(Vec::new()), None)?;
                self.scene.nodes[child].name = name;
                child
            };
            self.scene.nodes[target].mesh = Some(mesh);
            self.scene.nodes[target].skin = skin;
            if let Some(c) = morph {
                self.morph_nodes.entry(c).or_default().push(target);
            }
        }
        Ok(())
    }

    #[allow(clippy::type_complexity)]
    fn build_mesh(
        &mut self,
        members: &[(Rc<GeomData>, Rc<Binding<'a>>, Option<(Rc<SkinData<'a>>, usize, Option<usize>)>, Option<(usize, Rc<MorphData<'a>>)>)],
    ) -> Result<usize, MeshError> {
        let verts: usize = members.iter().flat_map(|m| &m.0.prims).map(|p| p.positions.len()).sum();
        let idx: usize = members.iter().flat_map(|m| &m.0.prims).map(|p| p.indices.len()).sum();
        self.total_vertices = self.total_vertices.saturating_add(verts);
        self.total_indices = self.total_indices.saturating_add(idx);
        if self.total_vertices > MAX_SCENE_VERTICES || self.total_indices > MAX_SCENE_INDICES {
            return Err(unsupported(format!(
                "the document instances more geometry than the scene's {MAX_SCENE_VERTICES}-vertex / \
                 {MAX_SCENE_INDICES}-index limits"
            )));
        }
        let mut mesh = SceneMesh { name: members.first().and_then(|m| m.0.name.clone()), ..Default::default() };
        for (geom, binding, skin, morph) in members {
            let morph_offsets = match morph {
                Some((_, md)) => Some(self.morph_offsets(md, geom)?),
                None => None,
            };
            if let Some((_, md)) = morph {
                mesh.weights = md.weights.clone();
                mesh.target_names = md.names.clone();
            }
            for gp in &geom.prims {
                let material = match &gp.symbol {
                    None => None,
                    Some(sym) => match binding.entries.iter().find(|(s, _, _)| s == sym) {
                        Some((_, mat, inputs)) => self.material(mat, inputs, &gp.uv_sets, geom.double_sided)?,
                        None => match self.doc.by_id(sym).filter(|e| e.name == "material") {
                            Some(mat) => {
                                self.warn_once(
                                    &format!("unbound {sym}"),
                                    format!("material symbol {} is not bound by the instance; the material with that id was used", q(sym)),
                                );
                                self.material(mat, &[], &gp.uv_sets, geom.double_sided)?
                            }
                            None => {
                                self.warn_once(
                                    &format!("unbound {sym}"),
                                    format!("material symbol {} is bound to nothing; those triangles have no material", q(sym)),
                                );
                                None
                            }
                        },
                    },
                };
                let mut p = Primitive {
                    positions: gp.positions.clone(),
                    normals: gp.normals.clone(),
                    tangents: None,
                    uvs: gp.uvs.clone(),
                    colors: gp.colors.clone(),
                    skin: None,
                    indices: gp.indices.clone(),
                    material,
                    targets: Vec::new(),
                };
                if let Some((sd, _, bind_shape_joint)) = skin {
                    p.skin = Some(self.skin_weights(sd, gp, geom.vertex_count, *bind_shape_joint)?);
                }
                if let Some(offsets) = &morph_offsets {
                    for (positions, normals) in offsets {
                        let tp: Vec<[f32; 3]> = gp.vertex.iter().map(|v| positions[*v as usize]).collect();
                        let tn = normals.as_ref().map(|n| gp.vertex.iter().map(|v| n[*v as usize]).collect());
                        p.targets.push(MorphTarget { positions: tp, normals: tn });
                    }
                    if p.targets.iter().any(|t| t.normals.is_some()) {
                        for t in p.targets.iter_mut() {
                            if t.normals.is_none() {
                                t.normals = Some(vec![[0.0; 3]; p.positions.len()]);
                            }
                        }
                    }
                }
                mesh.primitives.push(p);
            }
        }
        self.scene.meshes.push(mesh);
        Ok(self.scene.meshes.len() - 1)
    }

    /// Per `<vertices>` element offsets of every morph target: position and,
    /// when both base and target keep normals in `<vertices>`, normal.
    #[allow(clippy::type_complexity)]
    fn morph_offsets(&mut self, md: &MorphData<'a>, base: &GeomData) -> Result<Vec<(Vec<[f32; 3]>, Option<Vec<[f32; 3]>>)>, MeshError> {
        let base_pos = self.vertices_stream(md.base, "POSITION")?.ok_or_else(|| malformed("a morph base has no POSITION"))?;
        let base_nrm = self.vertices_stream(md.base, "NORMAL")?;
        let mut out = Vec::with_capacity(md.targets.len());
        for t in &md.targets {
            let tid = t.attr("id").unwrap_or("");
            let tp = self.vertices_stream(t, "POSITION")?.ok_or_else(|| malformed(format!("morph target {} has no POSITION", q(tid))))?;
            if tp.len() != base_pos.len() || tp.len() != base.vertex_count {
                return Err(malformed(format!(
                    "morph target {} has {} vertices and its base {}",
                    q(tid),
                    tp.len(),
                    base_pos.len()
                )));
            }
            let positions: Vec<[f32; 3]> = if md.relative {
                tp
            } else {
                tp.iter().zip(&base_pos).map(|(t, b)| [t[0] - b[0], t[1] - b[1], t[2] - b[2]]).collect()
            };
            let normals = match (&base_nrm, self.vertices_stream(t, "NORMAL")?) {
                (Some(bn), Some(tn)) if tn.len() == bn.len() && bn.len() == base_pos.len() => Some(if md.relative {
                    tn
                } else {
                    tn.iter().zip(bn).map(|(t, b)| [t[0] - b[0], t[1] - b[1], t[2] - b[2]]).collect()
                }),
                _ => None,
            };
            out.push((positions, normals));
        }
        Ok(out)
    }

    /// One `<vertices>` input of a geometry, as plain triples.
    fn vertices_stream(&mut self, geom: &'a Element, semantic: &str) -> Result<Option<Vec<[f32; 3]>>, MeshError> {
        let Some(v) = kid(geom, "mesh").and_then(|m| kid(m, "vertices")) else { return Ok(None) };
        let Some(input) = kids(v, "input").find(|i| i.attr("semantic") == Some(semantic)) else { return Ok(None) };
        let src = self.doc.resolve(input.attr("source").unwrap_or(""), "source", &format!("<vertices> {semantic}"))?;
        let (vals, view) = self.stream_f32(src, 3, semantic)?;
        Ok(Some((0..view.count).map(|i| [vals[view.at(i, 0)], vals[view.at(i, 1)], vals[view.at(i, 2)]]).collect()))
    }

    fn skin_weights(&mut self, sd: &SkinData<'a>, gp: &GeomPrim, vertex_count: usize, bind_shape_joint: Option<usize>) -> Result<SkinWeights, MeshError> {
        if sd.starts.len() != vertex_count + 1 {
            return Err(malformed(format!(
                "skin {} weights {} vertices and its geometry has {vertex_count}",
                q(sd.name.as_deref().unwrap_or("")),
                sd.starts.len().saturating_sub(1)
            )));
        }
        let mut per_vertex: Vec<Vec<(u16, f32)>> = Vec::with_capacity(gp.vertex.len());
        let mut widest = 1usize;
        let mut clipped = 0usize;
        for &v in &gp.vertex {
            let v = v as usize;
            let mut infl: Vec<(u16, f32)> = Vec::new();
            for &(j, w) in &sd.pairs[sd.starts[v]..sd.starts[v + 1]] {
                let joint = if j < 0 {
                    match bind_shape_joint {
                        Some(b) => b,
                        None => continue,
                    }
                } else {
                    j as usize
                };
                infl.push((u16::try_from(joint).map_err(|_| malformed("a skin joint index exceeds 65535"))?, w));
            }
            if infl.len() > MAX_INFLUENCES {
                infl.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
                infl.truncate(MAX_INFLUENCES);
                clipped += 1;
            }
            widest = widest.max(infl.len());
            per_vertex.push(infl);
        }
        if clipped > 0 {
            self.warn(format!("{clipped} skinned vertices have more than {MAX_INFLUENCES} influences; the strongest were kept"));
        }
        let influences = widest.div_ceil(4) * 4;
        let mut joints = Vec::with_capacity(per_vertex.len() * influences);
        let mut weights = Vec::with_capacity(per_vertex.len() * influences);
        for infl in per_vertex {
            for k in 0..influences {
                let (j, w) = infl.get(k).copied().unwrap_or((0, 0.0));
                joints.push(j);
                weights.push(w);
            }
        }
        Ok(SkinWeights { influences, joints, weights })
    }

    /// The scene skin for a controller instanced with these skeleton roots,
    /// and the joint standing for the bind shape when `<v>` names joint -1.
    fn skin_for(
        &mut self,
        ctrl: &'a Element,
        sd: &SkinData<'a>,
        skeletons: &[String],
        instance_node: usize,
        geom: &GeomData,
    ) -> Result<(usize, Option<usize>), MeshError> {
        let roots: Vec<usize> = skeletons
            .iter()
            .filter_map(|url| self.doc.get(url))
            .filter_map(|e| self.nodes_by_element.get(&mat_ptr(e)).and_then(|v| v.first().copied()))
            .collect();
        if roots.len() < skeletons.len() {
            self.warn(format!(
                "skin {}: a <skeleton> names no node of the scene; its joints were looked up everywhere",
                q(ctrl.attr("id").unwrap_or(""))
            ));
        }
        let mut joints = Vec::with_capacity(sd.joints.len());
        for name in &sd.joints {
            let found = if sd.joints_are_ids {
                self.lookup_global(name, LookupKey::Id)
            } else {
                let scoped = if roots.is_empty() { None } else { self.lookup_scoped(&roots, name) };
                scoped
                    .or_else(|| self.lookup_global(name, LookupKey::Sid))
                    .or_else(|| self.lookup_global(name, LookupKey::Id))
                    .or_else(|| self.lookup_global(name, LookupKey::Name))
            };
            joints.push(found.ok_or_else(|| {
                malformed(format!("skin {} joint {} matches no node of the scene", q(ctrl.attr("id").unwrap_or("")), q(name)))
            })?);
        }
        let mut ibms = sd.ibms.clone();
        let needs_bind_shape = sd.pairs.iter().any(|(j, w)| *j < 0 && *w != 0.0) && geom.vertex_count > 0;
        let bind_shape_joint = if needs_bind_shape {
            // "-1 refers to the bind shape": the vertex follows the instancing
            // node itself, which the scene says with that node as a joint.
            joints.push(instance_node);
            ibms.push(sd.bind_shape);
            Some(joints.len() - 1)
        } else {
            None
        };
        // By content, not by element: an exporter (this one included) writes a
        // controller per primitive, and controllers binding the same joints
        // the same way are one skin, whose primitives belong in one mesh.
        let mut key = format!("{joints:?}\u{1}{:?}", sd.name);
        for m in &ibms {
            for v in m {
                let _ = write!(key, " {:x}", v.to_bits());
            }
        }
        let index = match self.skin_cache.get(&key) {
            Some(&s) => s,
            None => {
                let skeleton = if roots.len() == 1 { roots.first().copied() } else { None };
                self.scene.skins.push(Skin { name: sd.name.clone(), joints, inverse_bind_matrices: ibms, skeleton });
                let s = self.scene.skins.len() - 1;
                self.skin_cache.insert(key, s);
                s
            }
        };
        Ok((index, bind_shape_joint))
    }

    fn lookup_scoped(&mut self, roots: &[usize], name: &str) -> Option<usize> {
        if !self.subtree_sid.contains_key(roots) {
            // Breadth-first from each root in order: the spec's sid scoping,
            // and the first match is the nearest one.
            let mut map: HashMap<String, usize> = HashMap::new();
            let mut queue: std::collections::VecDeque<usize> = roots.iter().copied().collect();
            let mut seen: HashSet<usize> = HashSet::new();
            while let Some(n) = queue.pop_front() {
                if !seen.insert(n) {
                    continue;
                }
                if let Some(sid) = self.info.get(n).and_then(|i| i.sid) {
                    map.entry(sid.to_string()).or_insert(n);
                }
                if let Some(node) = self.scene.nodes.get(n) {
                    queue.extend(node.children.iter().copied());
                }
            }
            self.subtree_sid.insert(roots.to_vec(), Rc::new(map));
        }
        self.subtree_sid.get(roots).and_then(|m| m.get(name).copied())
    }

    fn lookup_global(&mut self, name: &str, by: LookupKey) -> Option<usize> {
        let table = match by {
            LookupKey::Sid => &mut self.global_sid,
            LookupKey::Id => &mut self.global_id,
            LookupKey::Name => &mut self.global_name,
        };
        if table.is_none() {
            let mut map = HashMap::new();
            for (i, info) in self.info.iter().enumerate() {
                let k = match by {
                    LookupKey::Sid => info.sid,
                    LookupKey::Id => info.id,
                    LookupKey::Name => info.name,
                };
                if let Some(k) = k {
                    map.entry(k.to_string()).or_insert(i);
                }
            }
            *table = Some(map);
        }
        table.as_ref().and_then(|m| m.get(name).copied())
    }

    // ── animation ───────────────────────────────────────────────────────────

    fn read_animations(&mut self) -> Result<(), MeshError> {
        let tops = self.doc.library("library_animations", "animation");
        if tops.is_empty() {
            return Ok(());
        }
        self.is_root = vec![false; self.scene.nodes.len()];
        for &r in &self.scene.roots {
            if let Some(x) = self.is_root.get_mut(r) {
                *x = true;
            }
        }
        // Every channel, and for every <animation> the channels under it. The
        // walk is depth-first, so an animation's own and descendant channels
        // are one contiguous run of `channels`: a [start, end) range each.
        // (Listing every channel under every enclosing animation instead cost
        // channels x nesting depth — gigabytes from a 20 MB document.)
        let mut channels: Vec<&'a Element> = Vec::new();
        let mut under: HashMap<usize, (usize, usize)> = HashMap::new();
        enum Walk<'a> {
            Enter(&'a Element),
            Leave(usize, usize),
        }
        let mut stack: Vec<Walk<'a>> = tops.iter().rev().map(|a| Walk::Enter(a)).collect();
        while let Some(w) = stack.pop() {
            match w {
                Walk::Leave(animation, start) => {
                    under.insert(animation, (start, channels.len()));
                }
                Walk::Enter(a) => {
                    let start = channels.len();
                    channels.extend(a.children.iter().filter(|c| c.name == "channel"));
                    stack.push(Walk::Leave(mat_ptr(a), start));
                    for c in a.children.iter().rev().filter(|c| c.name == "animation") {
                        stack.push(Walk::Enter(c));
                    }
                }
            }
        }
        let clips = self.doc.library("library_animation_clips", "animation_clip");
        let mut groups: Vec<(Option<String>, Vec<usize>)> = Vec::new();
        if clips.is_empty() {
            let name = if tops.len() == 1 { tops[0].attr("name").filter(|s| !s.is_empty()).map(str::to_string) } else { None };
            groups.push((name, (0..channels.len()).collect()));
        } else {
            // Clips may each instance the same large animation; every clip
            // then holds (and later resamples) all of its channels again.
            let mut referenced = 0usize;
            for clip_el in clips {
                let mut chans: BTreeSet<usize> = BTreeSet::new();
                for ia in kids(clip_el, "instance_animation") {
                    let url = ia.attr("url").unwrap_or("");
                    let a = self.doc.resolve(url, "animation", "animation_clip instance_animation")?;
                    if let Some(&(start, end)) = under.get(&mat_ptr(a)) {
                        referenced = referenced.saturating_add(end - start);
                        if referenced > MAX_CLIP_CHANNELS {
                            return Err(unsupported(format!(
                                "the animation clips reference more than {MAX_CLIP_CHANNELS} channels in total"
                            )));
                        }
                        chans.extend(start..end);
                    }
                }
                let name = clip_el.attr("name").or(clip_el.attr("id")).filter(|s| !s.is_empty()).map(str::to_string);
                groups.push((name, chans.into_iter().collect()));
            }
        }
        for (name, idx) in groups {
            let chans: Vec<&'a Element> = idx.iter().filter_map(|i| channels.get(*i).copied()).collect();
            let anim = self.animation(name, &chans)?;
            if !anim.channels.is_empty() {
                self.scene.animations.push(anim);
            }
        }
        Ok(())
    }

    fn animation(&mut self, name: Option<String>, chans: &[&'a Element]) -> Result<Animation, MeshError> {
        let mut per_node: BTreeMap<usize, Vec<NodeCurve>> = BTreeMap::new();
        let mut per_weights: BTreeMap<usize, Vec<WeightCurve>> = BTreeMap::new();
        for ch in chans {
            let target = ch.attr("target").unwrap_or("");
            let source = ch.attr("source").unwrap_or("");
            let at = format!("animation channel {}", q(target));
            match self.resolve_target(target)? {
                Target::Ignored(why) => {
                    self.warn_once(&format!("target {target}"), format!("{at}: {why}; it was skipped"));
                }
                Target::Transform { element, op, select, dim } => {
                    let sampler = self.doc.resolve(source, "sampler", &at)?;
                    let curve = Rc::new(self.curve(sampler, dim, &at)?);
                    for &ir in self.nodes_by_element.get(&element).into_iter().flatten() {
                        per_node.entry(ir).or_default().push((op, select, curve.clone()));
                    }
                }
                Target::Weights { controller, index, targets } => {
                    let sampler = self.doc.resolve(source, "sampler", &at)?;
                    let dim = if index.is_some() { 1 } else { targets };
                    let curve = Rc::new(self.curve(sampler, dim, &at)?);
                    let nodes = self.morph_nodes.get(&controller).cloned().unwrap_or_default();
                    if nodes.is_empty() {
                        self.warn_once(
                            &format!("unplaced {target}"),
                            format!("{at} animates the weights of a morph no node of the scene draws; it was skipped"),
                        );
                    }
                    for ir in nodes {
                        per_weights.entry(ir).or_default().push((index, curve.clone()));
                    }
                }
            }
        }
        let mut out = Animation { name, channels: Vec::new() };
        for (ir, curves) in per_node {
            self.node_channels(ir, &curves, &mut out)?;
        }
        for (ir, curves) in per_weights {
            self.weight_channel(ir, &curves, &mut out)?;
        }
        Ok(out)
    }

    /// Every morph controller's MORPH_WEIGHT source and array, by address.
    fn weight_arrays(&mut self) -> &HashMap<usize, (usize, usize)> {
        if self.weight_arrays.is_none() {
            let mut map = HashMap::new();
            for ctrl in self.doc.library("library_controllers", "controller") {
                let Some(m) = kid(ctrl, "morph") else { continue };
                let weights = kid(m, "targets")
                    .into_iter()
                    .flat_map(|t| kids(t, "input").collect::<Vec<_>>())
                    .find(|i| i.attr("semantic") == Some("MORPH_WEIGHT"))
                    .and_then(|i| i.attr("source"))
                    .and_then(|u| self.doc.get(u));
                let Some(src) = weights else { continue };
                let array = src.children.iter().find(|c| c.name == "float_array");
                let count = array.map_or(0, |a| a.text.split_ascii_whitespace().count());
                map.entry(mat_ptr(src)).or_insert((mat_ptr(ctrl), count));
                if let Some(a) = array {
                    map.entry(mat_ptr(a)).or_insert((mat_ptr(ctrl), count));
                }
            }
            self.weight_arrays = Some(map);
        }
        self.weight_arrays.get_or_insert_with(HashMap::new)
    }

    fn resolve_target(&mut self, target: &str) -> Result<Target, MeshError> {
        let (id, sids, select) = parse_target(target).ok_or_else(|| malformed(format!("animation target {} does not parse", q(target))))?;
        let Some(el) = self.doc.by_id(id) else {
            return Ok(Target::Ignored(format!("it names {}, which is not in the document", q(id))));
        };
        if el.name == "node" {
            if sids.len() != 1 {
                return Ok(Target::Ignored("it addresses something inside a node other than a transform".into()));
            }
            let Some(info) = self.nodes_by_element.get(&mat_ptr(el)).and_then(|v| v.first()).and_then(|i| self.info.get(*i)) else {
                return Ok(Target::Ignored("its node is not part of the visual scene".into()));
            };
            let Some(op) = info.ops.iter().position(|o| o.sid.as_deref() == Some(sids[0])) else {
                return Ok(Target::Ignored(format!("node {} has no transform with sid {}", q(id), q(sids[0]))));
            };
            let kind = info.ops[op].kind;
            let width = info.ops[op].values.len();
            let comp = match (&select, kind) {
                (Select::All, _) => None,
                (_, OpKind::LookAt) => return Ok(Target::Ignored("animated <lookat> is not supported".into())),
                (Select::Member(m), OpKind::Translate | OpKind::Scale) => match m.as_str() {
                    "X" => Some(0),
                    "Y" => Some(1),
                    "Z" => Some(2),
                    _ => return Err(malformed(format!("animation target {} selects member {}", q(target), q(m)))),
                },
                (Select::Member(m), OpKind::Rotate) => match m.as_str() {
                    "X" => Some(0),
                    "Y" => Some(1),
                    "Z" => Some(2),
                    "ANGLE" => Some(3),
                    _ => return Err(malformed(format!("animation target {} selects member {}", q(target), q(m)))),
                },
                (Select::Member(m), OpKind::Matrix) => {
                    return Err(malformed(format!("animation target {} selects member {} of a matrix", q(target), q(m))))
                }
                (Select::Index(ix), OpKind::Matrix) => match ix.as_slice() {
                    // (row)(column) of the matrix as written.
                    [r, c] if *r < 4 && *c < 4 => Some(r * 4 + c),
                    [i] if *i < 16 => Some(*i),
                    _ => return Err(malformed(format!("animation target {} indexes outside a 4x4 matrix", q(target)))),
                },
                (Select::Index(ix), _) => match ix.as_slice() {
                    [i] if *i < width => Some(*i),
                    _ => return Err(malformed(format!("animation target {} indexes outside its transform", q(target)))),
                },
            };
            let dim = if comp.is_some() { 1 } else { width };
            return Ok(Target::Transform { element: mat_ptr(el), op, select: comp, dim });
        }
        // Morph weights: the MORPH_WEIGHT array, or its <source>, by id.
        if matches!(el.name.as_str(), "float_array" | "source") {
            if let Some(&(controller, targets)) = self.weight_arrays().get(&mat_ptr(el)) {
                let index = match select {
                    Select::All => None,
                    Select::Index(ix) if ix.len() == 1 && ix[0] < targets => Some(ix[0]),
                    _ => return Err(malformed(format!("animation target {} selects outside the morph weights", q(target)))),
                };
                return Ok(Target::Weights { controller, index, targets });
            }
        }
        Ok(Target::Ignored(format!("it animates a <{}>, which the scene does not model", clip(&el.name, 30))))
    }

    fn curve(&mut self, sampler: &'a Element, dim: usize, at: &str) -> Result<Curve, MeshError> {
        let mut input = None;
        let mut output = None;
        let mut interp = None;
        let mut in_tan = None;
        let mut out_tan = None;
        for i in kids(sampler, "input") {
            let url = i.attr("source").unwrap_or("");
            let sem = i.attr("semantic").unwrap_or("");
            let slot = match sem {
                "INPUT" => &mut input,
                "OUTPUT" => &mut output,
                "INTERPOLATION" => &mut interp,
                "IN_TANGENT" => &mut in_tan,
                "OUT_TANGENT" => &mut out_tan,
                _ => continue,
            };
            *slot = Some(self.doc.resolve(url, "source", &format!("{at} {sem}"))?);
        }
        let input = input.ok_or_else(|| malformed(format!("{at}: sampler has no INPUT")))?;
        let output = output.ok_or_else(|| malformed(format!("{at}: sampler has no OUTPUT")))?;
        let (tv, tview) = self.stream_f32(input, 1, "animation INPUT")?;
        let times: Vec<f64> = (0..tview.count).map(|i| tv[tview.at(i, 0)] as f64).collect();
        if times.is_empty() {
            return Err(malformed(format!("{at}: sampler has no keys")));
        }
        if times.windows(2).any(|w| w[1] < w[0]) {
            return Err(malformed(format!("{at}: key times decrease")));
        }
        let (ov, oview) = self.stream_f64(output, dim, "animation OUTPUT")?;
        if oview.count != times.len() {
            return Err(malformed(format!("{at}: {} keys but {} output values", times.len(), oview.count)));
        }
        if oview.comps.len() != dim {
            return Err(malformed(format!("{at}: output has {} components where its target takes {dim}", oview.comps.len())));
        }
        let mut values = Vec::with_capacity(times.len() * dim);
        for k in 0..times.len() {
            for c in 0..dim {
                values.push(ov[oview.at(k, c)]);
            }
        }
        let modes: Vec<Interp> = match interp {
            None => vec![Interp::Linear; times.len()],
            Some(src) => {
                let (names, nview) = self.stream_names(src, "INTERPOLATION")?;
                if nview.count != times.len() {
                    return Err(malformed(format!("{at}: {} keys but {} interpolations", times.len(), nview.count)));
                }
                (0..nview.count)
                    .map(|i| match names[nview.at(i, 0)].as_str() {
                        "LINEAR" => Ok(Interp::Linear),
                        "STEP" => Ok(Interp::Step),
                        "BEZIER" => Ok(Interp::Bezier),
                        "HERMITE" => Ok(Interp::Hermite),
                        other => Err(unsupported(format!("{at}: {} interpolation is not supported", q(other)))),
                    })
                    .collect::<Result<_, _>>()?
            }
        };
        let curved = modes[..modes.len().saturating_sub(1)].iter().any(|m| matches!(m, Interp::Bezier | Interp::Hermite));
        let mut tangents = None;
        if curved {
            let (Some(i), Some(o)) = (in_tan, out_tan) else {
                return Err(malformed(format!("{at}: BEZIER/HERMITE keys without IN_TANGENT and OUT_TANGENT")));
            };
            let read = |r: &mut Self, src: &'a Element| -> Result<(Vec<f64>, usize), MeshError> {
                let (v, view) = r.stream_f64(src, 1, "animation tangent")?;
                if view.count != times.len() {
                    return Err(malformed(format!("{at}: {} keys but {} tangents", times.len(), view.count)));
                }
                let w = view.comps.len();
                if w != dim && w != dim + 1 && w != 2 * dim {
                    return Err(malformed(format!("{at}: tangents of {w} values for a {dim}-value output")));
                }
                let mut out = Vec::with_capacity(view.count * w);
                for k in 0..view.count {
                    for c in 0..w {
                        out.push(v[view.at(k, c)]);
                    }
                }
                Ok((out, w))
            };
            let (iv, iw) = read(self, i)?;
            let (ov2, ow) = read(self, o)?;
            if iw != ow {
                return Err(malformed(format!("{at}: in and out tangents differ in width")));
            }
            tangents = Some((iv, ov2, iw));
        }
        Ok(Curve { times, values, dim, modes, tangents })
    }

    /// Resampled TRS channels for one node from the curves animating it.
    fn node_channels(&mut self, ir: usize, curves: &[NodeCurve], out: &mut Animation) -> Result<(), MeshError> {
        let ops = self.info[ir].ops.clone();
        let all_step = curves.iter().all(|(_, _, c)| c.all_step());
        let whole_matrices = curves.iter().all(|(op, sel, _)| sel.is_none() && ops.get(*op).is_some_and(|o| o.kind == OpKind::Matrix));
        let splines = curves.iter().any(|(_, _, c)| c.has_spline());
        // Whole-matrix keys are decomposed and their TRS interpolated, which is
        // how three.js plays them; anything else is composed per sample, and
        // between samples nothing but dense keys can follow it.
        let grid = !all_step && (!whole_matrices || splines);
        let times = self.sample_times(curves.iter().map(|(_, _, c)| c.as_ref()), all_step, grid)?;
        let is_root = self.is_root.get(ir).copied().unwrap_or(false);
        let mut tr = Vec::with_capacity(times.len() * 3);
        let mut rot = Vec::with_capacity(times.len() * 4);
        let mut sc = Vec::with_capacity(times.len() * 3);
        let mut prev_q: Option<[f64; 4]> = None;
        let mut scratch = vec![0.0f64; 16];
        for &t in &times {
            let mut now: Vec<Op> = ops.as_ref().clone();
            for (op, sel, c) in curves {
                let Some(o) = now.get_mut(*op) else { continue };
                scratch.resize(c.dim, 0.0);
                c.eval(t as f64, &mut scratch);
                match sel {
                    None => {
                        let n = o.values.len().min(c.dim);
                        o.values[..n].copy_from_slice(&scratch[..n]);
                    }
                    Some(i) => {
                        if let Some(v) = o.values.get_mut(*i) {
                            *v = scratch[0];
                        }
                    }
                }
            }
            let mut m = compose_ops(&now)?;
            if is_root {
                if let Some(cm) = &self.correction {
                    m = math::mat_mul(cm, &m);
                }
            }
            let (t3, mut q4, s3) = math::decompose_near(&m, prev_q).ok_or_else(|| {
                unsupported(format!("node {ir} is animated into a sheared transform at t={t}, which TRS keys cannot express"))
            })?;
            if let Some(p) = prev_q {
                if math::quat_dot(p, q4) < 0.0 {
                    q4 = [-q4[0], -q4[1], -q4[2], -q4[3]];
                }
            }
            prev_q = Some(q4);
            tr.extend(t3.iter().map(|v| *v as f32));
            rot.extend(q4.iter().map(|v| *v as f32));
            sc.extend(s3.iter().map(|v| *v as f32));
        }
        let interpolation = if all_step { Interpolation::Step } else { Interpolation::Linear };
        out.channels.push(Channel { node: ir, path: AnimPath::Translation, interpolation, times: times.clone(), values: tr });
        out.channels.push(Channel { node: ir, path: AnimPath::Rotation, interpolation, times: times.clone(), values: rot });
        out.channels.push(Channel { node: ir, path: AnimPath::Scale, interpolation, times, values: sc });
        Ok(())
    }

    fn weight_channel(&mut self, ir: usize, curves: &[WeightCurve], out: &mut Animation) -> Result<(), MeshError> {
        let Some(mi) = self.scene.nodes.get(ir).and_then(|n| n.mesh) else { return Ok(()) };
        let defaults = self.scene.meshes[mi].weights.clone();
        let n = self.scene.meshes[mi].target_count();
        if n == 0 {
            return Ok(());
        }
        let all_step = curves.iter().all(|(_, c)| c.all_step());
        let splines = curves.iter().any(|(_, c)| c.has_spline());
        let times = self.sample_times(curves.iter().map(|(_, c)| c.as_ref()), all_step, splines && !all_step)?;
        let mut values = Vec::with_capacity(times.len() * n);
        let mut scratch = vec![0.0f64; n.max(1)];
        for &t in &times {
            let mut w: Vec<f32> = if defaults.len() == n { defaults.clone() } else { vec![0.0; n] };
            for (index, c) in curves {
                scratch.resize(c.dim, 0.0);
                c.eval(t as f64, &mut scratch);
                match index {
                    Some(i) => {
                        if let Some(x) = w.get_mut(*i) {
                            *x = scratch[0] as f32;
                        }
                    }
                    None => {
                        for (x, v) in w.iter_mut().zip(&scratch) {
                            *x = *v as f32;
                        }
                    }
                }
            }
            values.extend(w);
        }
        let interpolation = if all_step { Interpolation::Step } else { Interpolation::Linear };
        out.channels.push(Channel { node: ir, path: AnimPath::Weights, interpolation, times, values });
        Ok(())
    }

    /// Key times for resampled curves: every key; a point just before each
    /// discontinuity (STEP, or two keys at one time) so linear interpolation
    /// jumps there; and, when `grid`, regular samples between.
    fn sample_times<'c>(&mut self, curves: impl Iterator<Item = &'c Curve> + Clone, all_step: bool, grid: bool) -> Result<Vec<f32>, MeshError> {
        let mut times: Vec<f64> = Vec::new();
        for c in curves.clone() {
            times.extend_from_slice(&c.times);
            if !all_step {
                c.jumps(&mut times);
            }
        }
        if grid {
            let lo = times.iter().copied().fold(f64::INFINITY, f64::min);
            let hi = times.iter().copied().fold(f64::NEG_INFINITY, f64::max);
            if hi > lo {
                let steps = (((hi - lo) * RESAMPLE_FPS as f64).ceil() as usize).clamp(1, MAX_RESAMPLES);
                for i in 0..=steps {
                    times.push(lo + (hi - lo) * i as f64 / steps as f64);
                }
            }
        }
        times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
        let mut out: Vec<f32> = Vec::with_capacity(times.len());
        for t in times {
            let t = t as f32;
            if out.last().is_none_or(|p| t > *p) {
                out.push(t);
            }
        }
        self.total_keys = self.total_keys.saturating_add(out.len() * 3);
        if self.total_keys > MAX_ANIMATION_KEYS {
            return Err(unsupported(format!("the animation resamples past {MAX_ANIMATION_KEYS} keys")));
        }
        Ok(out)
    }

    fn apply_correction(&mut self) {
        let Some(c) = self.correction else { return };
        let animated: HashSet<usize> = self.scene.animations.iter().flat_map(|a| &a.channels).map(|c| c.node).collect();
        for &r in &self.scene.roots {
            let node = &mut self.scene.nodes[r];
            let _ = animated.contains(&r);
            node.transform = Transform::from_matrix(math::mat_mul(&c, &node.transform.matrix()));
        }
    }
}

#[derive(Clone, Copy)]
enum LookupKey {
    Sid,
    Id,
    Name,
}

enum Target {
    Transform { element: usize, op: usize, select: Option<usize>, dim: usize },
    Weights { controller: usize, index: Option<usize>, targets: usize },
    Ignored(String),
}

#[derive(Debug, PartialEq)]
enum Select {
    All,
    Member(String),
    Index(Vec<usize>),
}

/// `id/sid/…` with an optional `.member` or `(i)(j)` on the last part
/// (spec 1.4.1 §3 "COLLADA Target Addressing").
fn parse_target(t: &str) -> Option<(&str, Vec<&str>, Select)> {
    let t = t.trim();
    let mut parts: Vec<&str> = t.split('/').collect();
    let last = parts.pop()?;
    let (last, select) = if let Some(open) = last.find('(') {
        let mut ix = Vec::new();
        let mut rest = &last[open..];
        while let Some(r) = rest.strip_prefix('(') {
            let close = r.find(')')?;
            ix.push(r[..close].trim().parse::<usize>().ok()?);
            rest = &r[close + 1..];
        }
        if !rest.is_empty() || ix.is_empty() || ix.len() > 2 {
            return None;
        }
        (&last[..open], Select::Index(ix))
    } else if let Some(dot) = last.find('.') {
        (&last[..dot], Select::Member(last[dot + 1..].to_string()))
    } else {
        (last, Select::All)
    };
    parts.push(last);
    let id = parts.first().copied()?;
    if id.is_empty() {
        return None;
    }
    Some((id, parts[1..].to_vec(), select))
}

/// A curve animating one transform element of a node: (element index in the
/// node's ops, the component it drives or `None` for all, the curve).
type NodeCurve = (usize, Option<usize>, Rc<Curve>);

/// A curve animating morph weights: (target index or `None` for all, curve).
type WeightCurve = (Option<usize>, Rc<Curve>);

#[derive(Clone, Copy, PartialEq, Debug)]
enum Interp {
    Linear,
    Step,
    Bezier,
    Hermite,
}

/// One sampler, ready to evaluate.
struct Curve {
    times: Vec<f64>,
    values: Vec<f64>,
    dim: usize,
    modes: Vec<Interp>,
    /// In-tangents, out-tangents and their per-key width (`dim`: 1-D values;
    /// `dim + 1`: one time then a value per dimension; `2 * dim`: a
    /// (time, value) pair per dimension).
    tangents: Option<(Vec<f64>, Vec<f64>, usize)>,
}

impl Curve {
    fn all_step(&self) -> bool {
        self.times.len() < 2 || self.modes[..self.modes.len() - 1].iter().all(|m| *m == Interp::Step)
    }

    fn has_spline(&self) -> bool {
        self.modes[..self.modes.len().saturating_sub(1)].iter().any(|m| matches!(m, Interp::Bezier | Interp::Hermite))
    }

    /// Times just before each value jump, for linear resampling to land on.
    fn jumps(&self, out: &mut Vec<f64>) {
        for k in 0..self.times.len().saturating_sub(1) {
            let (t0, t1) = (self.times[k], self.times[k + 1]);
            let step = self.modes[k] == Interp::Step && t1 > t0;
            let duplicate = t1 == t0;
            if step {
                let eps = (STEP_EPSILON as f64).max(t1.abs() * 1e-6).min((t1 - t0) / 2.0);
                out.push(t1 - eps);
            } else if duplicate && k > 0 && self.times[k - 1] < t0 {
                let eps = (STEP_EPSILON as f64).max(t0.abs() * 1e-6).min((t0 - self.times[k - 1]) / 2.0);
                out.push(t0 - eps);
            }
        }
    }

    fn key(&self, k: usize) -> &[f64] {
        &self.values[k * self.dim..(k + 1) * self.dim]
    }

    fn eval(&self, t: f64, out: &mut [f64]) {
        let n = self.times.len();
        let dim = self.dim;
        if n == 1 || t <= self.times[0] {
            // Before the first key, the first value; with keys duplicated at
            // the start, the last of them.
            let k = self.times.iter().rposition(|x| *x <= t).unwrap_or(0);
            out[..dim].copy_from_slice(self.key(k));
            return;
        }
        if t >= self.times[n - 1] {
            out[..dim].copy_from_slice(self.key(n - 1));
            return;
        }
        // The last key at or before t: with duplicate times, the later one.
        let k = self.times.partition_point(|x| *x <= t) - 1;
        let (t0, t1) = (self.times[k], self.times[k + 1]);
        let (a, b) = (self.key(k), self.key(k + 1));
        match self.modes[k] {
            Interp::Step => out[..dim].copy_from_slice(a),
            Interp::Linear => {
                let u = if t1 > t0 { (t - t0) / (t1 - t0) } else { 0.0 };
                for d in 0..dim {
                    out[d] = a[d] + (b[d] - a[d]) * u;
                }
            }
            mode @ (Interp::Bezier | Interp::Hermite) => {
                let Some((tin, tout, w)) = &self.tangents else {
                    out[..dim].copy_from_slice(a);
                    return;
                };
                for d in 0..dim {
                    // Control points (time, value) of this dimension.
                    let pick = |tan: &[f64], key: usize| -> (Option<f64>, f64) {
                        let row = &tan[key * w..(key + 1) * w];
                        if *w == 2 * dim {
                            (Some(row[2 * d]), row[2 * d + 1])
                        } else if *w == dim + 1 {
                            (Some(row[0]), row[1 + d])
                        } else {
                            (None, row[d])
                        }
                    };
                    let (ot, ov) = pick(tout, k);
                    let (it, iv) = pick(tin, k + 1);
                    let third = (t1 - t0) / 3.0;
                    let (c0, c1) = if mode == Interp::Bezier {
                        ((ot.unwrap_or(t0 + third), ov), (it.unwrap_or(t1 - third), iv))
                    } else {
                        // Hermite tangents T relate to Bézier controls by
                        // T0 = 3(C0 − P0), T1 = 3(P1 − C1) (spec §4).
                        let (dt0, dv0) = (ot.unwrap_or(t1 - t0), ov);
                        let (dt1, dv1) = (it.unwrap_or(t1 - t0), iv);
                        ((t0 + dt0 / 3.0, a[d] + dv0 / 3.0), (t1 - dt1 / 3.0, b[d] - dv1 / 3.0))
                    };
                    out[d] = bezier_at(t, (t0, a[d]), c0, c1, (t1, b[d]));
                }
            }
        }
    }
}

/// The value of a 2-D cubic Bézier at time `t`, solving the time polynomial
/// by bisection. Control times are clamped into the segment so time stays
/// monotonic, which is what animation editors enforce too.
fn bezier_at(t: f64, p0: (f64, f64), c0: (f64, f64), c1: (f64, f64), p1: (f64, f64)) -> f64 {
    let x1 = c0.0.clamp(p0.0, p1.0);
    let x2 = c1.0.clamp(p0.0, p1.0);
    let bez = |a: f64, b: f64, c: f64, d: f64, s: f64| {
        let r = 1.0 - s;
        r * r * r * a + 3.0 * r * r * s * b + 3.0 * r * s * s * c + s * s * s * d
    };
    let (mut lo, mut hi) = (0.0f64, 1.0f64);
    for _ in 0..60 {
        let mid = 0.5 * (lo + hi);
        if bez(p0.0, x1, x2, p1.0, mid) < t {
            lo = mid;
        } else {
            hi = mid;
        }
    }
    bez(p0.1, c0.1, c1.1, p1.1, 0.5 * (lo + hi))
}

fn parse_f64_list(text: &str, what: &str) -> Result<Vec<f64>, MeshError> {
    text.split_ascii_whitespace()
        .take(64)
        .map(|t| t.parse::<f64>().ok().filter(|v| v.is_finite()).ok_or_else(|| malformed(format!("{what} holds {}, not a number", q(t)))))
        .collect()
}

fn matrix_from_row_major(v: &[f64]) -> Option<Mat4> {
    if v.len() != 16 {
        return None;
    }
    let mut m = [0.0f64; 16];
    m.copy_from_slice(v);
    Some(math::transpose(&m))
}

fn color4(text: &str, at: &str) -> Result<[f32; 4], MeshError> {
    let v: Vec<f32> = text
        .split_ascii_whitespace()
        .take(5)
        .map(|t| t.parse::<f32>().ok().filter(|x| x.is_finite()).ok_or_else(|| malformed(format!("{at} colour holds {}", q(t)))))
        .collect::<Result<_, _>>()?;
    match v.len() {
        3 => Ok([v[0], v[1], v[2], 1.0]),
        4 => Ok([v[0], v[1], v[2], v[3]]),
        n => Err(malformed(format!("{at} colour has {n} components"))),
    }
}

fn decode_hex(text: &str) -> Option<Vec<u8>> {
    let digits: Vec<u8> = text.bytes().filter(|b| !b.is_ascii_whitespace()).collect();
    if !digits.len().is_multiple_of(2) {
        return None;
    }
    let val = |c: u8| -> Option<u8> {
        match c {
            b'0'..=b'9' => Some(c - b'0'),
            b'a'..=b'f' => Some(c - b'a' + 10),
            b'A'..=b'F' => Some(c - b'A' + 10),
            _ => None,
        }
    };
    digits.chunks_exact(2).map(|p| Some(val(p[0])? << 4 | val(p[1])?)).collect()
}

/// `<double_sided>` anywhere in an `<extra>` of this element: GOOGLEEARTH,
/// MAYA, MAX3D and FCOLLADA all spell it the same.
fn double_sided_extra(e: &Element) -> bool {
    kids(e, "extra").any(|x| {
        x.descendants_named("double_sided").iter().any(|d| matches!(d.text.trim(), "1" | "true" | "TRUE" | "True"))
    })
}

/// Wrap modes from the MAYA `<extra>` of a `<texture>` (what three.js reads).
fn maya_wrap(tex: &Element) -> Option<(Wrap, Wrap)> {
    let tech = kids(tex, "extra").flat_map(|x| kids(x, "technique").collect::<Vec<_>>()).find(|t| t.attr("profile") == Some("MAYA"))?;
    let flag = |name: &str| kid(tech, name).map(|e| matches!(e.text.trim(), "1" | "true" | "TRUE"));
    let axis = |wrap: &str, mirror: &str| match (flag(wrap), flag(mirror)) {
        (_, Some(true)) => Some(Wrap::MirroredRepeat),
        (Some(false), _) => Some(Wrap::ClampToEdge),
        (Some(true), _) => Some(Wrap::Repeat),
        _ => None,
    };
    let (s, t) = (axis("wrapU", "mirrorU"), axis("wrapV", "mirrorV"));
    if s.is_none() && t.is_none() {
        return None;
    }
    Some((s.unwrap_or_default(), t.unwrap_or_default()))
}

/// The scene's "no filter specified" sampler (what glTF's reader produces for
/// a sampler without `minFilter`): mipmapped, readers' defaults.
fn unspecified_sampler() -> Sampler {
    Sampler { mipmaps: true, ..Default::default() }
}

fn sampler_of(s2d: &Element, at: &str, warnings: &mut Vec<String>) -> Sampler {
    let mut out = unspecified_sampler();
    let wrap = |e: Option<&Element>, warnings: &mut Vec<String>| -> Wrap {
        match e.map(|e| e.text.trim()) {
            None | Some("WRAP") => Wrap::Repeat,
            Some("MIRROR") => Wrap::MirroredRepeat,
            Some("CLAMP") => Wrap::ClampToEdge,
            Some(other) => {
                warnings.push(clip(&format!("{at}: wrap mode {} is read as clamp", q(other)), MAX_DETAIL));
                Wrap::ClampToEdge
            }
        }
    };
    out.wrap_s = wrap(kid(s2d, "wrap_s"), warnings);
    out.wrap_t = wrap(kid(s2d, "wrap_t"), warnings);
    let filter = |name: &str| -> Option<Filter> {
        match name {
            "NEAREST" => Some(Filter::Nearest),
            "LINEAR" => Some(Filter::Linear),
            _ => None,
        }
    };
    if let Some(m) = kid(s2d, "magfilter") {
        out.mag_filter = filter(m.text.trim());
    }
    if let Some(m) = kid(s2d, "minfilter") {
        let (min, mip) = match m.text.trim() {
            "NEAREST" => (Some(Filter::Nearest), None),
            "LINEAR" => (Some(Filter::Linear), None),
            "NEAREST_MIPMAP_NEAREST" => (Some(Filter::Nearest), Some(Filter::Nearest)),
            "LINEAR_MIPMAP_NEAREST" => (Some(Filter::Linear), Some(Filter::Nearest)),
            "NEAREST_MIPMAP_LINEAR" => (Some(Filter::Nearest), Some(Filter::Linear)),
            "LINEAR_MIPMAP_LINEAR" => (Some(Filter::Linear), Some(Filter::Linear)),
            _ => (None, None),
        };
        out.min_filter = min;
        // 1.4.1 may also name the mip filter on its own element.
        let mip = mip.or_else(|| kid(s2d, "mipfilter").and_then(|e| filter(e.text.trim())));
        out.mipmaps = mip.is_some();
        if let Some(f) = mip {
            out.mipmap_filter = f;
        }
        if min.is_none() {
            out.mipmaps = true;
        }
    }
    out
}

/// A node's transform elements, in document order.
fn parse_ops(node: &Element) -> Result<Vec<Op>, MeshError> {
    let mut ops = Vec::new();
    let nid = node.attr("id").unwrap_or("");
    for c in &node.children {
        let kind = match c.name.as_str() {
            "matrix" => OpKind::Matrix,
            "translate" => OpKind::Translate,
            "rotate" => OpKind::Rotate,
            "scale" => OpKind::Scale,
            "lookat" => OpKind::LookAt,
            "skew" => {
                return Err(unsupported(format!(
                    "node {} has a <skew> transform, which this converter does not model",
                    q(nid)
                )))
            }
            _ => continue,
        };
        let want = match kind {
            OpKind::Matrix => 16,
            OpKind::Rotate => 4,
            OpKind::LookAt => 9,
            OpKind::Translate | OpKind::Scale => 3,
        };
        let values = parse_f64_list(&c.text, &format!("node {} <{}>", q(nid), c.name))?;
        if values.len() != want {
            return Err(malformed(format!("node {} <{}> has {} values, expected {want}", q(nid), c.name, values.len())));
        }
        if ops.len() >= MAX_NODE_OPS {
            return Err(unsupported(format!(
                "node {} has more than {MAX_NODE_OPS} transform elements; every animated sample recomposes them all",
                q(nid)
            )));
        }
        ops.push(Op { sid: c.attr("sid").map(str::to_string), kind, values });
    }
    Ok(ops)
}

/// The local matrix of a transform stack: each element post-multiplied, so
/// the first written is the outermost (spec 1.4.1 §5 `<node>`).
fn compose_ops(ops: &[Op]) -> Result<Mat4, MeshError> {
    let mut m = math::IDENTITY;
    for op in ops {
        let v = &op.values;
        let local = match op.kind {
            OpKind::Matrix => matrix_from_row_major(v).unwrap_or(math::IDENTITY),
            OpKind::Translate => math::translation([v[0], v[1], v[2]]),
            OpKind::Scale => math::scaling([v[0], v[1], v[2]]),
            OpKind::Rotate => {
                let axis = [v[0], v[1], v[2]];
                let len = (axis[0] * axis[0] + axis[1] * axis[1] + axis[2] * axis[2]).sqrt();
                if !(len > 1e-12) {
                    if v[3] == 0.0 {
                        math::IDENTITY
                    } else {
                        return Err(malformed("a <rotate> has a zero axis and a non-zero angle"));
                    }
                } else {
                    math::quat_to_mat(math::quat_from_axis_angle(axis, v[3].to_radians()))
                }
            }
            OpKind::LookAt => look_at(v)?,
        };
        m = math::mat_mul(&m, &local);
    }
    Ok(m)
}

/// `<lookat>` eye, interest, up: the object placed at the eye with its −Z
/// toward the interest point and +Y toward up — the inverse of the classic
/// gluLookAt view matrix, which is how the spec defines it for cameras.
fn look_at(v: &[f64]) -> Result<Mat4, MeshError> {
    let eye = [v[0], v[1], v[2]];
    let f = [v[3] - eye[0], v[4] - eye[1], v[5] - eye[2]];
    let up = [v[6], v[7], v[8]];
    let norm = |a: [f64; 3]| -> Option<[f64; 3]> {
        let l = (a[0] * a[0] + a[1] * a[1] + a[2] * a[2]).sqrt();
        (l > 1e-12 && l.is_finite()).then(|| [a[0] / l, a[1] / l, a[2] / l])
    };
    let cross = |a: [f64; 3], b: [f64; 3]| [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]];
    let f = norm(f).ok_or_else(|| malformed("a <lookat> looks at its own position"))?;
    let s = norm(cross(f, up)).ok_or_else(|| malformed("a <lookat> up vector is parallel to its view"))?;
    let u = cross(s, f);
    Ok([s[0], s[1], s[2], 0.0, u[0], u[1], u[2], 0.0, -f[0], -f[1], -f[2], 0.0, eye[0], eye[1], eye[2], 1.0])
}

enum Triangulation {
    /// Convex (or degenerate): fan from the first corner.
    Fan,
    Ears(Vec<[usize; 3]>),
    /// Concave and larger than [`MAX_EAR_CLIP`].
    TooLarge,
}

/// Triangles for one planar-ish polygon, keeping its winding. Concave
/// polygons are ear-clipped: a fan of a concave polygon covers area outside
/// it and leaves notches uncovered, which is a wrong model, not a cosmetic.
fn triangulate(pts: &[[f64; 3]]) -> Triangulation {
    let n = pts.len();
    // Newell's normal, robust for non-planar input.
    let mut nrm = [0.0f64; 3];
    for i in 0..n {
        let (a, b) = (pts[i], pts[(i + 1) % n]);
        nrm[0] += (a[1] - b[1]) * (a[2] + b[2]);
        nrm[1] += (a[2] - b[2]) * (a[0] + b[0]);
        nrm[2] += (a[0] - b[0]) * (a[1] + b[1]);
    }
    let len = (nrm[0] * nrm[0] + nrm[1] * nrm[1] + nrm[2] * nrm[2]).sqrt();
    if !(len > 1e-20) {
        return Triangulation::Fan;
    }
    // Project onto the plane most facing the normal, oriented so the polygon
    // is counter-clockwise in 2-D.
    let axis = if nrm[0].abs() >= nrm[1].abs() && nrm[0].abs() >= nrm[2].abs() {
        0
    } else if nrm[1].abs() >= nrm[2].abs() {
        1
    } else {
        2
    };
    let (u, v) = match axis {
        0 => (1, 2),
        1 => (2, 0),
        _ => (0, 1),
    };
    let sign = if nrm[axis] < 0.0 { -1.0 } else { 1.0 };
    let p2: Vec<[f64; 2]> = pts.iter().map(|p| [p[u], p[v] * sign]).collect();
    let cross = |o: [f64; 2], a: [f64; 2], b: [f64; 2]| (a[0] - o[0]) * (b[1] - o[1]) - (a[1] - o[1]) * (b[0] - o[0]);
    let convex = (0..n).all(|i| cross(p2[(i + n - 1) % n], p2[i], p2[(i + 1) % n]) >= -1e-12 * len);
    if convex {
        return Triangulation::Fan;
    }
    if n > MAX_EAR_CLIP {
        return Triangulation::TooLarge;
    }
    let mut ring: Vec<usize> = (0..n).collect();
    let mut out = Vec::with_capacity(n - 2);
    let inside = |p: [f64; 2], a: [f64; 2], b: [f64; 2], c: [f64; 2]| {
        cross(a, b, p) >= 0.0 && cross(b, c, p) >= 0.0 && cross(c, a, p) >= 0.0
    };
    let mut guard = 0;
    while ring.len() > 3 && guard < n * n {
        guard += 1;
        let m = ring.len();
        let mut clipped = false;
        for k in 0..m {
            let (ia, ib, ic) = (ring[(k + m - 1) % m], ring[k], ring[(k + 1) % m]);
            let (a, b, c) = (p2[ia], p2[ib], p2[ic]);
            if cross(a, b, c) <= 0.0 {
                continue;
            }
            if ring.iter().any(|&j| j != ia && j != ib && j != ic && inside(p2[j], a, b, c)) {
                continue;
            }
            out.push([ia, ib, ic]);
            ring.remove(k);
            clipped = true;
            break;
        }
        if !clipped {
            // Self-intersecting: no ear exists. Fan what is left.
            break;
        }
    }
    for k in 1..ring.len().saturating_sub(1) {
        out.push([ring[0], ring[k], ring[k + 1]]);
    }
    Triangulation::Ears(out)
}

// ─── reader: entry points ───────────────────────────────────────────────────

/// Read a COLLADA document into a scene. Images are looked up in `sidecars`.
pub fn read_scene(bytes: &[u8], sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    if bytes.len() > super::MAX_INPUT_BYTES {
        return Err(unsupported(format!("input is {} bytes, over the {}-byte limit", bytes.len(), super::MAX_INPUT_BYTES)));
    }
    sniff(bytes)?;
    let root = super::xml::parse(bytes).map_err(|e| match e {
        MeshError::Malformed(d) => malformed(format!("the XML does not parse ({d})")),
        other => other,
    })?;
    if root.name != "COLLADA" {
        return Err(not_dae(format!("the XML root element is <{}>, not <COLLADA>", clip(&root.name, 60))));
    }
    if let Some(v) = root.attr("version") {
        if !(v.starts_with("1.4") || v.starts_with("1.5")) {
            return Err(unsupported(format!("COLLADA version {}; only 1.4 and 1.5 documents are read", q(v))));
        }
    }
    let doc = Doc::new(&root);
    let duplicates = doc.duplicate_ids;
    let mut r = Reader::new(doc, sidecars);
    if duplicates > 0 {
        r.warn(format!("{duplicates} element id(s) repeat; each reference resolves to the first"));
    }
    r.read_asset()?;
    r.read_nodes()?;
    r.assemble()?;
    r.read_animations()?;
    r.apply_correction();
    let Reader { scene, warnings, .. } = r;
    scene.validate()?;
    if scene.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    Ok(SceneRead { scene, warnings })
}

/// Read a COLLADA document into the flat mesh (see `pose::flatten`).
pub fn read(bytes: &[u8]) -> Result<Mesh, MeshError> {
    let r = read_scene(bytes, &NoSidecars)?;
    let mesh = pose::flatten(&r.scene)?;
    if mesh.indices.is_empty() {
        return Err(MeshError::Empty);
    }
    Ok(mesh)
}

// ─── writer ─────────────────────────────────────────────────────────────────

/// Unique, deterministic XML ids. Every id in the document comes from here,
/// including the ones derived from others (`…-positions`), so a node that
/// happens to be called `box-mesh-positions` cannot collide with a source.
struct Ids {
    used: HashSet<String>,
}

impl Ids {
    /// An `xs:ID`-safe id from a free-text name: ASCII letters, digits, `_`,
    /// `-` and `.`, starting with a letter or `_`. ASCII only, because the
    /// joint ids double as `Name_array` tokens that every reader splits on
    /// whitespace and compares byte for byte.
    fn make(&mut self, base: &str, fallback: &str) -> String {
        let mut s: String = base
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() || matches!(c, '_' | '-' | '.') { c } else { '_' })
            .take(64)
            .collect();
        if s.is_empty() {
            s = fallback.to_string();
        }
        if !s.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_') {
            s.insert(0, '_');
        }
        let mut candidate = s.clone();
        let mut n = 2;
        while !self.used.insert(candidate.clone()) {
            candidate = format!("{s}-{n}");
            n += 1;
        }
        candidate
    }
}

/// A `name` attribute for a free-text name: 1.4.1 types every `name`
/// `xs:NCName` (1.5 relaxed it to free text), and readers act on it — SceneKit
/// cuts a name at its first space ("Left Arm" loads as "Left"), and OpenCOLLADA,
/// the Max/Maya exporter, writes NCNames for that reason. Every character an
/// NCName cannot hold becomes `_`; letters of every script survive (XML 1.0
/// 5th edition name characters, which libxml2's schema validation accepts).
/// `None` for an empty name: no attribute at all.
fn ncname(name: Option<&str>) -> Option<String> {
    let name = name.filter(|n| !n.is_empty())?;
    let start = |c: char| {
        matches!(c, 'A'..='Z' | '_' | 'a'..='z' | '\u{C0}'..='\u{D6}' | '\u{D8}'..='\u{F6}' | '\u{F8}'..='\u{2FF}'
            | '\u{370}'..='\u{37D}' | '\u{37F}'..='\u{1FFF}' | '\u{200C}'..='\u{200D}' | '\u{2070}'..='\u{218F}'
            | '\u{2C00}'..='\u{2FEF}' | '\u{3001}'..='\u{D7FF}' | '\u{F900}'..='\u{FDCF}' | '\u{FDF0}'..='\u{FFFD}'
            | '\u{10000}'..='\u{EFFFF}')
    };
    let rest = |c: char| start(c) || matches!(c, '-' | '.' | '0'..='9' | '\u{B7}' | '\u{300}'..='\u{36F}' | '\u{203F}'..='\u{2040}');
    let mut out: String = name.chars().map(|c| if rest(c) { c } else { '_' }).collect();
    if !out.starts_with(start) {
        out.insert(0, '_');
    }
    Some(out)
}

/// Escape for attribute values and text. Tabs and line breaks become
/// character references: XML attribute-value normalisation would otherwise
/// turn a name's newline into a space on the way back in. Other control
/// characters cannot be carried by XML 1.0 at all and are dropped.
fn esc(v: &str) -> String {
    let mut out = String::with_capacity(v.len());
    for c in v.chars() {
        match c {
            '&' => out.push_str("&amp;"),
            '<' => out.push_str("&lt;"),
            '>' => out.push_str("&gt;"),
            '"' => out.push_str("&quot;"),
            '\'' => out.push_str("&apos;"),
            '\t' => out.push_str("&#9;"),
            '\n' => out.push_str("&#10;"),
            '\r' => out.push_str("&#13;"),
            c if (c as u32) < 0x20 => {}
            c => out.push(c),
        }
    }
    out
}

/// An indenting XML writer. Text is written inline with no surrounding
/// whitespace: three.js takes `<init_from>`'s text content verbatim as a path.
struct Out {
    s: String,
    depth: usize,
}

impl Out {
    fn indent(&mut self) {
        for _ in 0..self.depth {
            self.s.push_str("  ");
        }
    }

    fn tag(&mut self, tag: &str, attrs: &[(&str, &str)]) {
        self.s.push('<');
        self.s.push_str(tag);
        for (k, v) in attrs {
            let _ = write!(self.s, " {k}=\"{}\"", esc(v));
        }
    }

    fn open(&mut self, tag: &str, attrs: &[(&str, &str)]) {
        self.indent();
        self.tag(tag, attrs);
        self.s.push_str(">\n");
        self.depth += 1;
    }

    fn close(&mut self, tag: &str) {
        self.depth = self.depth.saturating_sub(1);
        self.indent();
        let _ = writeln!(self.s, "</{tag}>");
    }

    fn empty(&mut self, tag: &str, attrs: &[(&str, &str)]) {
        self.indent();
        self.tag(tag, attrs);
        self.s.push_str("/>\n");
    }

    /// `<tag>text</tag>`, with `text` escaped here.
    fn leaf(&mut self, tag: &str, attrs: &[(&str, &str)], text: &str) {
        self.indent();
        self.tag(tag, attrs);
        let _ = writeln!(self.s, ">{}</{tag}>", esc(text));
    }

    /// `<tag>numbers</tag>`, the numbers already formatted.
    fn numbers(&mut self, tag: &str, attrs: &[(&str, &str)], text: &str) {
        self.indent();
        self.tag(tag, attrs);
        let _ = writeln!(self.s, ">{text}</{tag}>");
    }
}

/// Space-separated shortest round-trip decimals. Rust's `Display` for floats
/// writes the shortest text that parses back to the same bits and never
/// uses exponents, both of which Collada's `xs:float` lists accept.
fn join_f32(vals: impl IntoIterator<Item = f32>) -> String {
    let mut s = String::new();
    for v in vals {
        if !s.is_empty() {
            s.push(' ');
        }
        let _ = write!(s, "{v}");
    }
    s
}

fn join_f64(vals: impl IntoIterator<Item = f64>) -> String {
    let mut s = String::new();
    for v in vals {
        if !s.is_empty() {
            s.push(' ');
        }
        let _ = write!(s, "{v}");
    }
    s
}

fn wrap_name(w: Wrap) -> &'static str {
    match w {
        Wrap::Repeat => "WRAP",
        Wrap::ClampToEdge => "CLAMP",
        Wrap::MirroredRepeat => "MIRROR",
    }
}

/// Key times for a baked matrix channel: every source key and 30 fps between
/// (`pose::resample_times`), plus a key just before each STEP key, holding
/// the old value, so that a reader interpolating linearly — three.js does on
/// every matrix channel — jumps where the source steps instead of ramping.
fn bake_times(scene: &Scene, anim: usize) -> Vec<f32> {
    let mut times = pose::resample_times(scene, anim, RESAMPLE_FPS);
    if let Some(a) = scene.animations.get(anim) {
        for ch in a.channels.iter().filter(|c| c.interpolation == Interpolation::Step && c.path != AnimPath::Weights) {
            for w in ch.times.windows(2) {
                let (t0, t1) = (w[0], w[1]);
                let eps = STEP_EPSILON.max(t1.abs() * 4.0 * f32::EPSILON).min((t1 - t0) / 2.0);
                let before = t1 - eps;
                if before > t0 && before < t1 {
                    times.push(before);
                }
            }
        }
    }
    times.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    times.dedup();
    times
}

struct Writer<'s> {
    s: &'s Scene,
    /// Package mode: textures go out as sidecar files.
    images: bool,
    ids: Ids,
    out: Out,
    scene_id: String,
    node_ids: Vec<String>,
    /// A joint that holds a mesh draws it from a child node: three.js builds
    /// a leaf node with exactly one instance AS that mesh, not as a bone, and
    /// the skeleton would lose the joint.
    wrapper_ids: Vec<Option<String>>,
    /// Nodes for a skinned mesh's second and later skinned primitives:
    /// SceneKit's importer keeps one `<instance_controller>` per node and
    /// silently drops the rest (measured: a two-controller node loads with
    /// one skinned geometry). Skinning ignores the holding node's transform,
    /// so an identity child changes nothing for any reader.
    skin_part_ids: Vec<Vec<String>>,
    is_joint: Vec<bool>,
    image_ids: Vec<Option<String>>,
    image_paths: Vec<Option<String>>,
    material_ids: Vec<String>,
    effect_ids: Vec<String>,
    geom_ids: Vec<Vec<String>>,
    /// (mesh, skin) → a controller per primitive that carries weights.
    controllers: BTreeMap<(usize, usize), Vec<Option<String>>>,
    files: Vec<PackageFile>,
}

impl<'s> Writer<'s> {
    fn new(s: &'s Scene, images: bool) -> Writer<'s> {
        Writer {
            s,
            images,
            ids: Ids { used: HashSet::new() },
            out: Out { s: String::new(), depth: 0 },
            scene_id: String::new(),
            node_ids: Vec::new(),
            wrapper_ids: Vec::new(),
            skin_part_ids: Vec::new(),
            is_joint: Vec::new(),
            image_ids: Vec::new(),
            image_paths: Vec::new(),
            material_ids: Vec::new(),
            effect_ids: Vec::new(),
            geom_ids: Vec::new(),
            controllers: BTreeMap::new(),
            files: Vec::new(),
        }
    }

    /// Allocate every library id up front, in a fixed order, so the same
    /// scene always gets the same ids and the nodes get first pick of their
    /// own names.
    fn plan(&mut self) {
        let s = self.s;
        self.scene_id = self.ids.make("Scene", "Scene");
        self.is_joint = s.joint_nodes();
        for (i, n) in s.nodes.iter().enumerate() {
            let id = self.ids.make(n.name.as_deref().unwrap_or(""), &format!("node{i}"));
            self.node_ids.push(id);
        }
        for (i, n) in s.nodes.iter().enumerate() {
            let w = (self.is_joint[i] && n.mesh.is_some())
                .then(|| self.ids.make(&format!("{}-geometry", self.node_ids[i]), "geometry"));
            self.wrapper_ids.push(w);
            let skinned = match (n.mesh, n.skin) {
                (Some(m), Some(_)) => s.meshes[m].primitives.iter().filter(|p| p.skin.is_some()).count(),
                _ => 0,
            };
            let parts = (1..skinned).map(|k| self.ids.make(&format!("{}-skin{k}", self.node_ids[i]), "skin_part")).collect();
            self.skin_part_ids.push(parts);
        }
        let mut used_paths: HashSet<String> = HashSet::new();
        for (i, im) in s.images.iter().enumerate() {
            if !self.images || !im.format.is_universal() {
                self.image_ids.push(None);
                self.image_paths.push(None);
                continue;
            }
            let stem = package::safe_file_stem(im.name.as_deref().unwrap_or(""), &format!("image_{i}"));
            let mut path = format!("{TEXTURE_DIR}/{stem}.{}", im.format.extension());
            let mut n = 2;
            while !used_paths.insert(path.clone()) {
                path = format!("{TEXTURE_DIR}/{stem}_{n}.{}", im.format.extension());
                n += 1;
            }
            let id = self.ids.make(&format!("{stem}-image"), "image");
            self.image_ids.push(Some(id));
            self.image_paths.push(Some(path));
        }
        for (i, m) in s.materials.iter().enumerate() {
            let base = m.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("material{i}"));
            let mid = self.ids.make(&format!("{base}-material"), "material");
            let eid = self.ids.make(&format!("{base}-effect"), "effect");
            self.material_ids.push(mid);
            self.effect_ids.push(eid);
        }
        for (mi, m) in s.meshes.iter().enumerate() {
            let base = m.name.clone().filter(|n| !n.is_empty()).unwrap_or_else(|| format!("mesh{mi}"));
            let mut ids = Vec::with_capacity(m.primitives.len());
            for pi in 0..m.primitives.len() {
                let b = if m.primitives.len() > 1 { format!("{base}-{pi}") } else { base.clone() };
                ids.push(self.ids.make(&format!("{b}-mesh"), "mesh"));
            }
            self.geom_ids.push(ids);
        }
        for n in &s.nodes {
            let (Some(mi), Some(si)) = (n.mesh, n.skin) else { continue };
            if self.controllers.contains_key(&(mi, si)) {
                continue;
            }
            let per_prim = s.meshes[mi]
                .primitives
                .iter()
                .enumerate()
                .map(|(pi, p)| p.skin.as_ref().map(|_| self.ids.make(&format!("{}-skin", self.geom_ids[mi][pi]), "skin")))
                .collect();
            self.controllers.insert((mi, si), per_prim);
        }
    }

    fn document(mut self) -> Result<Package, MeshError> {
        self.plan();
        self.out.s.push_str("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n");
        self.out.open("COLLADA", &[("xmlns", NS_COLLADA_141), ("version", "1.4.1")]);
        self.asset();
        if self.image_ids.iter().any(Option::is_some) {
            self.library_images();
        }
        if !self.s.materials.is_empty() {
            self.library_effects();
            self.library_materials();
        }
        self.library_geometries();
        if self.controllers.values().flatten().any(Option::is_some) {
            self.library_controllers();
        }
        let clips = self.library_animations();
        if !clips.is_empty() {
            self.library_animation_clips(&clips);
        }
        self.library_visual_scenes();
        self.out.open("scene", &[]);
        let url = format!("#{}", self.scene_id);
        self.out.empty("instance_visual_scene", &[("url", &url)]);
        self.out.close("scene");
        self.out.close("COLLADA");
        let mut pkg = Package::single(PRIMARY_FILE, std::mem::take(&mut self.out.s).into_bytes());
        pkg.files.append(&mut self.files);
        Ok(pkg)
    }

    fn asset(&mut self) {
        let o = &mut self.out;
        o.open("asset", &[]);
        o.open("contributor", &[]);
        o.leaf("authoring_tool", &[], AUTHORING_TOOL);
        o.close("contributor");
        o.leaf("created", &[], TIMESTAMP);
        o.leaf("modified", &[], TIMESTAMP);
        o.empty("unit", &[("name", "meter"), ("meter", "1")]);
        o.leaf("up_axis", &[], "Y_UP");
        o.close("asset");
    }

    fn library_images(&mut self) {
        self.out.open("library_images", &[]);
        for (i, im) in self.s.images.iter().enumerate() {
            let (Some(id), Some(path)) = (self.image_ids[i].clone(), self.image_paths[i].clone()) else { continue };
            let mut attrs = vec![("id", id.as_str())];
            let name_text = ncname(im.name.as_deref());
            if let Some(n) = name_text.as_deref() {
                attrs.push(("name", n));
            }
            self.out.open("image", &attrs);
            self.out.leaf("init_from", &[], &path);
            self.out.close("image");
            self.files.push(PackageFile { path, bytes: im.data.clone() });
        }
        self.out.close("library_images");
    }

    /// The image id a texture's sampler can name, when this package carries
    /// that image.
    fn texture_image(&self, texture: usize) -> Option<&str> {
        let t = self.s.textures.get(texture)?;
        self.image_ids.get(t.image)?.as_deref()
    }

    fn library_effects(&mut self) {
        self.out.open("library_effects", &[]);
        for mi in 0..self.s.materials.len() {
            self.effect(mi);
        }
        self.out.close("library_effects");
    }

    fn effect(&mut self, mi: usize) {
        let m = &self.s.materials[mi];
        let eid = self.effect_ids[mi].clone();
        let mut attrs = vec![("id", eid.as_str())];
        let name_text = ncname(m.name.as_deref());
        if let Some(n) = name_text.as_deref() {
            attrs.push(("name", n));
        }
        self.out.open("effect", &attrs);
        self.out.open("profile_COMMON", &[]);
        // One surface and sampler per texture the effect samples.
        let mut samplers: Vec<(usize, String)> = Vec::new();
        for r in [&m.base_color_texture, &m.emissive_texture].into_iter().flatten() {
            if samplers.iter().any(|(t, _)| *t == r.texture) {
                continue;
            }
            let Some(image) = self.texture_image(r.texture).map(str::to_string) else { continue };
            let k = samplers.len();
            let surface = format!("texture{k}-surface");
            let sampler = format!("texture{k}-sampler");
            let sm = &self.s.textures[r.texture].sampler;
            self.out.open("newparam", &[("sid", &surface)]);
            self.out.open("surface", &[("type", "2D")]);
            self.out.leaf("init_from", &[], &image);
            self.out.close("surface");
            self.out.close("newparam");
            self.out.open("newparam", &[("sid", &sampler)]);
            self.out.open("sampler2D", &[]);
            self.out.leaf("source", &[], &surface);
            self.out.leaf("wrap_s", &[], wrap_name(sm.wrap_s));
            self.out.leaf("wrap_t", &[], wrap_name(sm.wrap_t));
            if let Some(f) = sm.min_filter {
                let name = match (f, sm.mipmaps) {
                    (Filter::Nearest, false) => "NEAREST",
                    (Filter::Linear, false) => "LINEAR",
                    (Filter::Nearest, true) if sm.mipmap_filter == Filter::Nearest => "NEAREST_MIPMAP_NEAREST",
                    (Filter::Linear, true) if sm.mipmap_filter == Filter::Nearest => "LINEAR_MIPMAP_NEAREST",
                    (Filter::Nearest, true) => "NEAREST_MIPMAP_LINEAR",
                    (Filter::Linear, true) => "LINEAR_MIPMAP_LINEAR",
                };
                self.out.leaf("minfilter", &[], name);
            }
            if let Some(f) = sm.mag_filter {
                self.out.leaf("magfilter", &[], if f == Filter::Nearest { "NEAREST" } else { "LINEAR" });
            }
            self.out.close("sampler2D");
            self.out.close("newparam");
            samplers.push((r.texture, sampler));
        }
        let sampler_for = |r: &Option<TextureRef>| -> Option<(String, u32, usize)> {
            let r = r.as_ref()?;
            samplers.iter().find(|(t, _)| *t == r.texture).map(|(_, s)| (s.clone(), r.uv_set, r.texture))
        };
        let diffuse_tex = sampler_for(&m.base_color_texture);
        let emissive_tex = sampler_for(&m.emissive_texture);
        self.out.open("technique", &[("sid", "common")]);
        let shader = if m.roughness >= 1.0 { "lambert" } else { "phong" };
        self.out.open(shader, &[]);
        self.out.open("emission", &[]);
        match &emissive_tex {
            Some(t) => self.texture_element(t),
            None => {
                let e = m.emissive.map(srgb_encode);
                self.out.numbers("color", &[("sid", "emission")], &join_f32([e[0], e[1], e[2], 1.0]));
            }
        }
        self.out.close("emission");
        self.out.open("diffuse", &[]);
        match &diffuse_tex {
            Some(t) => self.texture_element(t),
            None => {
                let c = [srgb_encode(m.base_color[0]), srgb_encode(m.base_color[1]), srgb_encode(m.base_color[2]), 1.0];
                self.out.numbers("color", &[("sid", "diffuse")], &join_f32(c));
            }
        }
        self.out.close("diffuse");
        if shader == "phong" {
            // A dielectric's 4% reflectance; the scene's metallic does not
            // survive a Phong model (and is reported by `scene::losses`).
            let k = srgb_encode(0.04);
            self.out.open("specular", &[]);
            self.out.numbers("color", &[("sid", "specular")], &join_f32([k, k, k, 1.0]));
            self.out.close("specular");
            self.out.open("shininess", &[]);
            self.out.numbers("float", &[("sid", "shininess")], &join_f32([shininess_from_roughness(m.roughness)]));
            self.out.close("shininess");
        }
        if m.alpha_mode == AlphaMode::Blend {
            // A_ONE: opacity = transparent.a × transparency (spec §8); with a
            // texture, its alpha is the transparent channel.
            self.out.open("transparent", &[("opaque", "A_ONE")]);
            match &diffuse_tex {
                Some(t) => self.texture_element(t),
                None => self.out.numbers("color", &[], "1 1 1 1"),
            }
            self.out.close("transparent");
            self.out.open("transparency", &[]);
            self.out.numbers("float", &[("sid", "transparency")], &join_f32([m.base_color[3]]));
            self.out.close("transparency");
        }
        self.out.close(shader);
        if m.double_sided {
            // Read by three.js (`technique.extra.technique.double_sided`) and
            // assimp; GOOGLEEARTH is the profile SketchUp and Blender wrote.
            self.out.open("extra", &[]);
            self.out.open("technique", &[("profile", "GOOGLEEARTH")]);
            self.out.numbers("double_sided", &[], "1");
            self.out.close("technique");
            self.out.close("extra");
        }
        self.out.close("technique");
        self.out.close("profile_COMMON");
        self.out.close("effect");
    }

    /// `<texture>` naming a sampler and its coordinate set, with the MAYA
    /// wrap flags when the wrap is not plain repeat (see [`capabilities`]).
    fn texture_element(&mut self, (sampler, uv_set, texture): &(String, u32, usize)) {
        let texcoord = format!("UVSET{uv_set}");
        let sm = &self.s.textures[*texture].sampler;
        if sm.wrap_s == Wrap::Repeat && sm.wrap_t == Wrap::Repeat {
            self.out.empty("texture", &[("texture", sampler), ("texcoord", &texcoord)]);
            return;
        }
        let flag = |b: bool| if b { "1" } else { "0" };
        self.out.open("texture", &[("texture", sampler), ("texcoord", &texcoord)]);
        self.out.open("extra", &[]);
        self.out.open("technique", &[("profile", "MAYA")]);
        self.out.numbers("wrapU", &[], flag(sm.wrap_s != Wrap::ClampToEdge));
        self.out.numbers("wrapV", &[], flag(sm.wrap_t != Wrap::ClampToEdge));
        self.out.numbers("mirrorU", &[], flag(sm.wrap_s == Wrap::MirroredRepeat));
        self.out.numbers("mirrorV", &[], flag(sm.wrap_t == Wrap::MirroredRepeat));
        self.out.close("technique");
        self.out.close("extra");
        self.out.close("texture");
    }

    fn library_materials(&mut self) {
        self.out.open("library_materials", &[]);
        for (mi, m) in self.s.materials.iter().enumerate() {
            let mid = self.material_ids[mi].clone();
            let mut attrs = vec![("id", mid.as_str())];
            let name_text = ncname(m.name.as_deref());
            if let Some(n) = name_text.as_deref() {
                attrs.push(("name", n));
            }
            self.out.open("material", &attrs);
            let url = format!("#{}", self.effect_ids[mi]);
            self.out.empty("instance_effect", &[("url", &url)]);
            self.out.close("material");
        }
        self.out.close("library_materials");
    }

    /// A `<source>` with its array and accessor; returns the source id.
    #[allow(clippy::too_many_arguments)]
    fn source(&mut self, base: &str, array: &str, text: &str, values: usize, count: usize, stride: usize, params: &[(&str, &str)]) -> String {
        let sid = self.ids.make(base, "source");
        let aid = self.ids.make(&format!("{sid}-array"), "array");
        self.out.open("source", &[("id", &sid)]);
        self.out.numbers(array, &[("id", &aid), ("count", &values.to_string())], text);
        self.out.open("technique_common", &[]);
        let url = format!("#{aid}");
        self.out.open("accessor", &[("source", &url), ("count", &count.to_string()), ("stride", &stride.to_string())]);
        for (name, ty) in params {
            self.out.empty("param", &[("name", name), ("type", ty)]);
        }
        self.out.close("accessor");
        self.out.close("technique_common");
        self.out.close("source");
        sid
    }

    fn library_geometries(&mut self) {
        self.out.open("library_geometries", &[]);
        for mi in 0..self.s.meshes.len() {
            for pi in 0..self.s.meshes[mi].primitives.len() {
                self.geometry(mi, pi);
            }
        }
        self.out.close("library_geometries");
    }

    fn geometry(&mut self, mi: usize, pi: usize) {
        let mesh = &self.s.meshes[mi];
        let p = &mesh.primitives[pi];
        let gid = self.geom_ids[mi][pi].clone();
        let mut attrs = vec![("id", gid.as_str())];
        let name_text = ncname(mesh.name.as_deref());
        if let Some(n) = name_text.as_deref() {
            attrs.push(("name", n));
        }
        self.out.open("geometry", &attrs);
        self.out.open("mesh", &[]);
        let n = p.positions.len();
        let xyz = [("X", "float"), ("Y", "float"), ("Z", "float")];
        let positions = self.source(&format!("{gid}-positions"), "float_array", &join_f32(p.positions.iter().flatten().copied()), n * 3, n, 3, &xyz);
        let normals = p
            .normals
            .as_ref()
            .map(|ns| self.source(&format!("{gid}-normals"), "float_array", &join_f32(ns.iter().flatten().copied()), n * 3, n, 3, &xyz));
        let mut uv_sources = Vec::new();
        for (set, uvs) in p.uvs.iter().enumerate().take(UV_SETS) {
            // V flipped to Collada's bottom-left origin. Both values in f64:
            // the f32 `u` exactly, and `1 - v` exactly, so the reader's
            // `1 - (1 - v)` restores v to the bit.
            let text = join_f64(uvs.iter().flat_map(|t| [t[0] as f64, 1.0 - t[1] as f64]));
            let st = [("S", "float"), ("T", "float")];
            uv_sources.push((set, self.source(&format!("{gid}-map-{set}"), "float_array", &text, n * 2, n, 2, &st)));
        }
        let colors = p.colors.as_ref().map(|cs| {
            let rgba = [("R", "float"), ("G", "float"), ("B", "float"), ("A", "float")];
            self.source(&format!("{gid}-colors"), "float_array", &join_f32(cs.iter().flatten().copied()), n * 4, n, 4, &rgba)
        });
        let vid = self.ids.make(&format!("{gid}-vertices"), "vertices");
        self.out.open("vertices", &[("id", &vid)]);
        self.out.empty("input", &[("semantic", "POSITION"), ("source", &format!("#{positions}"))]);
        if let Some(c) = &colors {
            // In <vertices>: the one place three.js reads colours unconverted
            // (see the module docs).
            self.out.empty("input", &[("semantic", "COLOR"), ("source", &format!("#{c}"))]);
        }
        self.out.close("vertices");
        let count = (p.indices.len() / 3).to_string();
        let mut tattrs = vec![("count", count.as_str())];
        let symbol = p.material.map(|m| self.material_ids[m].clone());
        if let Some(sym) = &symbol {
            tattrs.push(("material", sym));
        }
        self.out.open("triangles", &tattrs);
        self.out.empty("input", &[("semantic", "VERTEX"), ("source", &format!("#{vid}")), ("offset", "0")]);
        if let Some(nid) = &normals {
            self.out.empty("input", &[("semantic", "NORMAL"), ("source", &format!("#{nid}")), ("offset", "0")]);
        }
        for (set, sid) in &uv_sources {
            self.out.empty(
                "input",
                &[("semantic", "TEXCOORD"), ("source", &format!("#{sid}")), ("offset", "0"), ("set", &set.to_string())],
            );
        }
        let mut idx = String::with_capacity(p.indices.len() * 6);
        for (k, i) in p.indices.iter().enumerate() {
            if k > 0 {
                idx.push(' ');
            }
            let _ = write!(idx, "{i}");
        }
        self.out.numbers("p", &[], &idx);
        self.out.close("triangles");
        self.out.close("mesh");
        self.out.close("geometry");
    }

    fn library_controllers(&mut self) {
        self.out.open("library_controllers", &[]);
        let entries: Vec<(usize, usize, Vec<Option<String>>)> =
            self.controllers.iter().map(|((m, s), v)| (*m, *s, v.clone())).collect();
        for (mi, si, per_prim) in entries {
            for (pi, cid) in per_prim.into_iter().enumerate() {
                if let Some(cid) = cid {
                    self.controller(&cid, mi, pi, si);
                }
            }
        }
        self.out.close("library_controllers");
    }

    fn controller(&mut self, cid: &str, mi: usize, pi: usize, si: usize) {
        let s = self.s;
        let p = &s.meshes[mi].primitives[pi];
        let skin = &s.skins[si];
        let Some(sw) = &p.skin else { return };
        let gid = self.geom_ids[mi][pi].clone();
        let mut attrs = vec![("id", cid)];
        let name_text = ncname(skin.name.as_deref());
        if let Some(n) = name_text.as_deref() {
            attrs.push(("name", n));
        }
        self.out.open("controller", &attrs);
        self.out.open("skin", &[("source", &format!("#{gid}"))]);
        self.out.numbers("bind_shape_matrix", &[], &join_f64(row_major(&math::IDENTITY)));
        let names: Vec<&str> = skin.joints.iter().map(|j| self.node_ids[*j].as_str()).collect();
        let names = names.join(" ");
        let jn = skin.joints.len();
        let joints = self.source(&format!("{cid}-joints"), "Name_array", &names, jn, jn, 1, &[("JOINT", "name")]);
        let ibm_text = join_f64(skin.inverse_bind_matrices.iter().flat_map(row_major));
        let ibms = self.source(&format!("{cid}-bind_poses"), "float_array", &ibm_text, jn * 16, jn, 16, &[("TRANSFORM", "float4x4")]);
        // Influences: positive weights, normalised, one weight entry each. A
        // vertex with none is bound wholly to its first listed joint — what
        // the scene's skinning does with it (`pose::skin_in_place`).
        let mut vcount = String::new();
        let mut v = String::new();
        let mut weights: Vec<f32> = Vec::new();
        for vi in 0..p.positions.len() {
            let (js, ws) = sw.vertex(vi);
            let sum: f64 = ws.iter().filter(|w| **w > 0.0).map(|w| *w as f64).sum();
            let mut infl: Vec<(u16, f32)> = Vec::new();
            if sum > 0.0 && sum.is_finite() {
                for (j, w) in js.iter().zip(ws) {
                    if *w > 0.0 {
                        infl.push((*j, (*w as f64 / sum) as f32));
                    }
                }
            } else {
                infl.push((js.first().copied().unwrap_or(0), 1.0));
            }
            if !vcount.is_empty() {
                vcount.push(' ');
            }
            let _ = write!(vcount, "{}", infl.len());
            for (j, w) in infl {
                if !v.is_empty() {
                    v.push(' ');
                }
                let _ = write!(v, "{j} {}", weights.len());
                weights.push(w);
            }
        }
        let wn = weights.len();
        let wsrc = self.source(&format!("{cid}-weights"), "float_array", &join_f32(weights), wn, wn, 1, &[("WEIGHT", "float")]);
        self.out.open("joints", &[]);
        self.out.empty("input", &[("semantic", "JOINT"), ("source", &format!("#{joints}"))]);
        self.out.empty("input", &[("semantic", "INV_BIND_MATRIX"), ("source", &format!("#{ibms}"))]);
        self.out.close("joints");
        self.out.open("vertex_weights", &[("count", &p.positions.len().to_string())]);
        self.out.empty("input", &[("semantic", "JOINT"), ("source", &format!("#{joints}")), ("offset", "0")]);
        self.out.empty("input", &[("semantic", "WEIGHT"), ("source", &format!("#{wsrc}")), ("offset", "1")]);
        self.out.numbers("vcount", &[], &vcount);
        self.out.numbers("v", &[], &v);
        self.out.close("vertex_weights");
        self.out.close("skin");
        self.out.close("controller");
    }

    /// One `<animation>` per animated node and clip, each a baked matrix
    /// channel; returns, per clip, the animations it groups.
    fn library_animations(&mut self) -> Vec<(usize, Vec<String>)> {
        let s = self.s;
        let mut clips = Vec::new();
        let mut open = false;
        for (ai, a) in s.animations.iter().enumerate() {
            let nodes: BTreeSet<usize> =
                a.channels.iter().filter(|c| c.path != AnimPath::Weights).map(|c| c.node).collect();
            if nodes.is_empty() {
                continue;
            }
            if !open {
                self.out.open("library_animations", &[]);
                open = true;
            }
            let times = bake_times(s, ai);
            let mut mats: Vec<Vec<f64>> = vec![Vec::with_capacity(times.len() * 16); nodes.len()];
            for &t in &times {
                let pose = pose::sample_pose(s, ai, t);
                for (k, n) in nodes.iter().enumerate() {
                    let m = match pose.trs.get(*n).copied().flatten() {
                        // three.js decomposes every matrix key, dividing by
                        // each axis' length: an exactly-zero scale (a part
                        // scaled in from nothing) becomes a NaN rotation and
                        // the part vanishes for good. A millionth renders the
                        // same and decomposes.
                        Some((tr, rot, sc)) => math::compose(tr, rot, math::nonsingular_scale(sc)),
                        None => pose.matrices.get(*n).copied().unwrap_or(math::IDENTITY),
                    };
                    mats[k].extend(row_major(&m));
                }
            }
            let mut ids = Vec::new();
            for (k, n) in nodes.iter().enumerate() {
                let node_id = self.node_ids[*n].clone();
                let aid = self.ids.make(&format!("{node_id}-anim"), "anim");
                self.out.open("animation", &[("id", &aid)]);
                let count = times.len();
                let input = self.source(&format!("{aid}-input"), "float_array", &join_f32(times.iter().copied()), count, count, 1, &[("TIME", "float")]);
                let output = self.source(
                    &format!("{aid}-output"),
                    "float_array",
                    &join_f64(mats[k].iter().copied()),
                    count * 16,
                    count,
                    16,
                    &[("TRANSFORM", "float4x4")],
                );
                let interp_text = vec!["LINEAR"; count].join(" ");
                let interp = self.source(&format!("{aid}-interpolation"), "Name_array", &interp_text, count, count, 1, &[("INTERPOLATION", "name")]);
                let sampler = self.ids.make(&format!("{aid}-sampler"), "sampler");
                self.out.open("sampler", &[("id", &sampler)]);
                self.out.empty("input", &[("semantic", "INPUT"), ("source", &format!("#{input}"))]);
                self.out.empty("input", &[("semantic", "OUTPUT"), ("source", &format!("#{output}"))]);
                self.out.empty("input", &[("semantic", "INTERPOLATION"), ("source", &format!("#{interp}"))]);
                self.out.close("sampler");
                self.out.empty("channel", &[("source", &format!("#{sampler}")), ("target", &format!("{node_id}/transform"))]);
                self.out.close("animation");
                ids.push(aid);
            }
            clips.push((ai, ids));
        }
        if open {
            self.out.close("library_animations");
        }
        clips
    }

    fn library_animation_clips(&mut self, clips: &[(usize, Vec<String>)]) {
        self.out.open("library_animation_clips", &[]);
        for (ai, ids) in clips {
            let a = &self.s.animations[*ai];
            let (lo, hi) = a.time_range().unwrap_or((0.0, 0.0));
            let cid = self.ids.make(a.name.as_deref().unwrap_or(""), &format!("animation{ai}"));
            // The clip starts where playback does, at 0, not at its first key:
            // three.js takes `end - start` as the clip's duration
            // (ColladaComposer.buildAnimationClip) and clamps playback there,
            // so a clip whose keys begin at 1/24 s (CesiumMan) would stop
            // 1/24 s short of its last key.
            let (start, end) = (lo.min(0.0).to_string(), hi.to_string());
            let mut attrs = vec![("id", cid.as_str()), ("start", start.as_str()), ("end", end.as_str())];
            let name_text = ncname(a.name.as_deref());
            if let Some(n) = name_text.as_deref() {
                attrs.push(("name", n));
            }
            self.out.open("animation_clip", &attrs);
            for id in ids {
                self.out.empty("instance_animation", &[("url", &format!("#{id}"))]);
            }
            self.out.close("animation_clip");
        }
        self.out.close("library_animation_clips");
    }

    fn library_visual_scenes(&mut self) {
        let s = self.s;
        self.out.open("library_visual_scenes", &[]);
        let sid = self.scene_id.clone();
        let mut attrs = vec![("id", sid.as_str())];
        let name_text = ncname(s.name.as_deref());
        if let Some(n) = name_text.as_deref() {
            attrs.push(("name", n));
        }
        self.out.open("visual_scene", &attrs);
        enum Step {
            Open(usize),
            Close,
        }
        // Explicit stack: depth is whatever the scene has.
        let mut stack: Vec<Step> = s.roots.iter().rev().map(|r| Step::Open(*r)).collect();
        while let Some(step) = stack.pop() {
            let i = match step {
                Step::Close => {
                    self.out.close("node");
                    continue;
                }
                Step::Open(i) => i,
            };
            let n = &s.nodes[i];
            let id = self.node_ids[i].clone();
            let mut attrs = vec![("id", id.as_str())];
            let name_text = ncname(n.name.as_deref());
            if let Some(name) = name_text.as_deref() {
                attrs.push(("name", name));
            }
            if self.is_joint[i] {
                attrs.push(("sid", id.as_str()));
                attrs.push(("type", "JOINT"));
            } else {
                attrs.push(("type", "NODE"));
            }
            self.out.open("node", &attrs);
            // A node scaled to nothing keeps a millionth, as its keys do:
            // readers decompose this matrix (`math::nonsingular_scale`).
            let m = match n.transform.trs() {
                Some((t, r, s)) => math::compose(t, r, math::nonsingular_scale(s)),
                None => n.transform.matrix(),
            };
            self.out.numbers("matrix", &[("sid", "transform")], &join_f64(row_major(&m)));
            match self.wrapper_ids[i].clone() {
                Some(w) => {
                    let mut wattrs = vec![("id", w.as_str())];
                    let name_text = ncname(n.name.as_deref());
                    if let Some(name) = name_text.as_deref() {
                        wattrs.push(("name", name));
                    }
                    wattrs.push(("type", "NODE"));
                    self.out.open("node", &wattrs);
                    self.out.numbers("matrix", &[("sid", "transform")], &join_f64(row_major(&math::IDENTITY)));
                    self.instances(i);
                    self.out.close("node");
                }
                None => self.instances(i),
            }
            stack.push(Step::Close);
            for c in n.children.iter().rev() {
                stack.push(Step::Open(*c));
            }
        }
        self.out.close("visual_scene");
        self.out.close("library_visual_scenes");
    }

    /// The skeleton roots of a skin: its joints with no ancestor among its
    /// joints. three.js and assimp search below each `<skeleton>` for the
    /// joints' sids, so these must cover every joint, and disjointly.
    fn skeleton_roots(&self, si: usize) -> Vec<usize> {
        let s = self.s;
        let joints: HashSet<usize> = s.skins[si].joints.iter().copied().collect();
        let parents = s.parents();
        let mut roots = Vec::new();
        for &j in &s.skins[si].joints {
            let mut cur = parents.get(j).copied().flatten();
            let mut steps = 0;
            let mut covered = false;
            while let Some(p) = cur {
                if joints.contains(&p) {
                    covered = true;
                    break;
                }
                steps += 1;
                if steps > s.nodes.len() {
                    break;
                }
                cur = parents.get(p).copied().flatten();
            }
            if !covered && !roots.contains(&j) {
                roots.push(j);
            }
        }
        roots
    }

    fn instances(&mut self, ni: usize) {
        let s = self.s;
        let n = &s.nodes[ni];
        let Some(mi) = n.mesh else { return };
        let mesh = &s.meshes[mi];
        let per_prim = n.skin.and_then(|si| self.controllers.get(&(mi, si)).cloned());
        // Schema order: every instance_controller before any instance_geometry,
        // and child nodes last. The first skinned primitive is drawn here,
        // later ones from identity child nodes (see `skin_part_ids`).
        let mut later: Vec<(usize, String)> = Vec::new();
        if let (Some(si), Some(per_prim)) = (n.skin, &per_prim) {
            let roots = self.skeleton_roots(si);
            let mut k = 0;
            for (pi, cid) in per_prim.iter().enumerate() {
                let Some(cid) = cid else { continue };
                if k > 0 {
                    later.push((pi, cid.clone()));
                    k += 1;
                    continue;
                }
                k += 1;
                self.instance_controller(cid, &roots, &mesh.primitives[pi]);
            }
        }
        for (pi, p) in mesh.primitives.iter().enumerate() {
            if per_prim.as_ref().is_some_and(|v| v.get(pi).is_some_and(Option::is_some)) {
                continue;
            }
            let url = format!("#{}", self.geom_ids[mi][pi]);
            if p.material.is_some() {
                self.out.open("instance_geometry", &[("url", &url)]);
                self.bind_material(p);
                self.out.close("instance_geometry");
            } else {
                self.out.empty("instance_geometry", &[("url", &url)]);
            }
        }
        if let Some(si) = n.skin {
            let roots = self.skeleton_roots(si);
            for (k, (pi, cid)) in later.into_iter().enumerate() {
                let Some(part_id) = self.skin_part_ids[ni].get(k).cloned() else { continue };
                let mut attrs = vec![("id", part_id.as_str())];
                let name_text = ncname(n.name.as_deref());
                if let Some(name) = name_text.as_deref() {
                    attrs.push(("name", name));
                }
                attrs.push(("type", "NODE"));
                self.out.open("node", &attrs);
                self.out.numbers("matrix", &[("sid", "transform")], &join_f64(row_major(&math::IDENTITY)));
                self.instance_controller(&cid, &roots, &mesh.primitives[pi]);
                self.out.close("node");
            }
        }
    }

    fn instance_controller(&mut self, cid: &str, roots: &[usize], p: &Primitive) {
        self.out.open("instance_controller", &[("url", &format!("#{cid}"))]);
        for r in roots {
            let url = format!("#{}", self.node_ids[*r]);
            self.out.leaf("skeleton", &[], &url);
        }
        self.bind_material(p);
        self.out.close("instance_controller");
    }

    fn bind_material(&mut self, p: &Primitive) {
        let Some(m) = p.material else { return };
        let mat = &self.s.materials[m];
        let mid = self.material_ids[m].clone();
        let mut sets: BTreeSet<u32> = BTreeSet::new();
        for r in [&mat.base_color_texture, &mat.emissive_texture].into_iter().flatten() {
            if self.texture_image(r.texture).is_some() {
                sets.insert(r.uv_set);
            }
        }
        self.out.open("bind_material", &[]);
        self.out.open("technique_common", &[]);
        let target = format!("#{mid}");
        if sets.is_empty() {
            self.out.empty("instance_material", &[("symbol", &mid), ("target", &target)]);
        } else {
            self.out.open("instance_material", &[("symbol", &mid), ("target", &target)]);
            for set in sets {
                let set_s = set.to_string();
                self.out.empty(
                    "bind_vertex_input",
                    &[("semantic", &format!("UVSET{set}")), ("input_semantic", "TEXCOORD"), ("input_set", &set_s)],
                );
            }
            self.out.close("instance_material");
        }
        self.out.close("technique_common");
        self.out.close("bind_material");
    }
}

// ─── writer: entry points ───────────────────────────────────────────────────

/// Write a scene — already reduced by [`prepare`] when it comes through
/// `super::write_scene` — as `model.dae` plus, unless `self_contained`, its
/// textures under `textures/`.
pub fn write_scene(scene: &Scene, opts: WriteOptions) -> Result<Package, MeshError> {
    scene.validate()?;
    if scene.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    let hoisted = glb::hoist_skinned_meshes(scene);
    Writer::new(&hoisted, !opts.self_contained).document()
}

/// Write the flat mesh as one self-contained document.
pub fn write(mesh: &Mesh) -> Result<Vec<u8>, MeshError> {
    mesh.validate()?;
    if mesh.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    let mut pkg = write_scene(&pose::from_flat(mesh), WriteOptions::default())?;
    Ok(pkg.files.remove(0).bytes)
}

// ─── tests ──────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::fixtures;
    use super::super::package::MemorySidecars;

    // ── builders ────────────────────────────────────────────────────────────

    fn package(scene: &Scene, opts: WriteOptions) -> Package {
        write_scene(&prepare(scene, opts).unwrap(), opts).unwrap()
    }

    fn read_package(pkg: &Package) -> SceneRead {
        read_scene(&pkg.files[0].bytes, &pkg.resolver()).unwrap()
    }

    fn text(pkg: &Package) -> String {
        String::from_utf8(pkg.files[0].bytes.clone()).unwrap()
    }

    /// A document around library snippets.
    fn doc(body: &str) -> Vec<u8> {
        format!(
            r#"<?xml version="1.0" encoding="utf-8"?><COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1"><asset><created>x</created><modified>x</modified></asset>{body}</COLLADA>"#
        )
        .into_bytes()
    }

    fn read_doc(body: &str) -> Result<SceneRead, MeshError> {
        read_scene(&doc(body), &NoSidecars)
    }

    fn floats(v: &[f32]) -> String {
        v.iter().map(|x| x.to_string()).collect::<Vec<_>>().join(" ")
    }

    /// A geometry `id` with these positions and primitive elements, which
    /// reference its `<vertices>` as `#{id}-v`.
    fn geom(id: &str, pos: &[f32], prims: &str) -> String {
        format!(
            r##"<geometry id="{id}" name="{id}"><mesh><source id="{id}-p"><float_array id="{id}-pa" count="{}">{}</float_array><technique_common><accessor source="#{id}-pa" count="{}" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source><vertices id="{id}-v"><input semantic="POSITION" source="#{id}-p"/></vertices>{prims}</mesh></geometry>"##,
            pos.len(),
            floats(pos),
            pos.len() / 3
        )
    }

    fn scene_doc(geometries: &str, nodes: &str) -> String {
        format!(
            r##"<library_geometries>{geometries}</library_geometries><library_visual_scenes><visual_scene id="vs">{nodes}</visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##
        )
    }

    const TRI: [f32; 9] = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0, 0.0];

    fn tri_doc(extra_node_content: &str) -> String {
        scene_doc(
            &geom("g", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles>"##),
            &format!(r##"<node id="n">{extra_node_content}<instance_geometry url="#g"/></node>"##),
        )
    }

    fn world_positions(s: &Scene) -> Vec<[f32; 3]> {
        pose::world_primitives(s, &pose::default_pose(s)).unwrap().into_iter().flat_map(|w| w.positions).collect()
    }

    fn close3(a: [f32; 3], b: [f32; 3], eps: f32) -> bool {
        (0..3).all(|i| (a[i] - b[i]).abs() <= eps)
    }

    fn signed_area_z(s: &Scene) -> Vec<f32> {
        let mut out = Vec::new();
        for w in pose::world_primitives(s, &pose::default_pose(s)).unwrap() {
            for t in w.indices.chunks_exact(3) {
                let (a, b, c) = (w.positions[t[0] as usize], w.positions[t[1] as usize], w.positions[t[2] as usize]);
                out.push(((b[0] - a[0]) * (c[1] - a[1]) - (b[1] - a[1]) * (c[0] - a[0])) / 2.0);
            }
        }
        out
    }

    // ── round trips ─────────────────────────────────────────────────────────

    /// The scene the writer was handed, as the reader must give it back:
    /// `prepare`d, with skinned meshes at identity roots (the hoist is part of
    /// the encoding).
    fn written_form(fixture: &Scene, opts: WriteOptions) -> Scene {
        glb::hoist_skinned_meshes(&prepare(fixture, opts).unwrap())
    }

    fn assert_round_trip(name: &str, fixture: &Scene, opts: WriteOptions) {
        let want = written_form(fixture, opts);
        let got = read_package(&package(fixture, opts)).scene;
        let ctx = format!("{name} ({})", if opts.self_contained { "single file" } else { "package" });
        assert_eq!(got.nodes.len(), want.nodes.len(), "{ctx}: node count");
        for (i, (g, w)) in got.nodes.iter().zip(&want.nodes).enumerate() {
            assert_eq!(g.name, w.name, "{ctx}: node {i} name");
            assert_eq!(g.children, w.children, "{ctx}: node {i} children");
            assert_eq!(g.mesh.is_some(), w.mesh.is_some(), "{ctx}: node {i} mesh");
            assert_eq!(g.skin.is_some(), w.skin.is_some(), "{ctx}: node {i} skin");
            let (gm, wm) = (g.transform.matrix(), w.transform.matrix());
            assert!(gm.iter().zip(&wm).all(|(a, b)| (a - b).abs() < 1e-9), "{ctx}: node {i} transform {gm:?} vs {wm:?}");
        }
        assert_eq!(got.roots, want.roots, "{ctx}: roots");
        assert_eq!(got.meshes.len(), want.meshes.len(), "{ctx}: mesh count (instancing must survive)");
        for (mi, (g, w)) in got.meshes.iter().zip(&want.meshes).enumerate() {
            assert_eq!(g.name, w.name, "{ctx}: mesh {mi} name");
            assert_eq!(g.primitives.len(), w.primitives.len(), "{ctx}: mesh {mi} primitives");
            for (pi, (gp, wp)) in g.primitives.iter().zip(&w.primitives).enumerate() {
                let at = format!("{ctx}: mesh {mi} primitive {pi}");
                // Text, written shortest-round-trip, gives every float back
                // to the bit; UVs too, through the f64 flip.
                assert_eq!(gp.positions, wp.positions, "{at}: positions");
                assert_eq!(gp.normals, wp.normals, "{at}: normals");
                assert_eq!(gp.uvs, wp.uvs, "{at}: uvs");
                assert_eq!(gp.colors, wp.colors, "{at}: colours");
                assert_eq!(gp.indices, wp.indices, "{at}: indices");
                assert_eq!(gp.material, wp.material, "{at}: material");
                assert_eq!(gp.targets.len(), 0, "{at}: morph targets are never written");
                match (&gp.skin, &wp.skin) {
                    (None, None) => {}
                    (Some(gs), Some(ws)) => {
                        for v in 0..gp.positions.len() {
                            // Compared as normalised influence sets: the
                            // writer drops zero weights and normalises.
                            let norm = |js: &[u16], ws: &[f32]| -> Vec<(u16, i64)> {
                                let sum: f32 = ws.iter().filter(|w| **w > 0.0).sum();
                                let mut out: Vec<(u16, i64)> = js
                                    .iter()
                                    .zip(ws)
                                    .filter(|(_, w)| **w > 0.0)
                                    .map(|(j, w)| (*j, (w / sum * 1e5).round() as i64))
                                    .collect();
                                out.sort();
                                out
                            };
                            let (gj, gw) = gs.vertex(v);
                            let (wj, ww) = ws.vertex(v);
                            assert_eq!(norm(gj, gw), norm(wj, ww), "{at}: vertex {v} influences");
                        }
                    }
                    other => panic!("{at}: skin presence differs: {other:?}"),
                }
            }
        }
        assert_eq!(got.materials.len(), want.materials.len(), "{ctx}: material count");
        for (i, (g, w)) in got.materials.iter().zip(&want.materials).enumerate() {
            let at = format!("{ctx}: material {i}");
            assert_eq!(g.name, w.name, "{at}");
            for k in 0..4 {
                assert!((g.base_color[k] - w.base_color[k]).abs() < 1e-6, "{at}: base colour {:?} vs {:?}", g.base_color, w.base_color);
            }
            for k in 0..3 {
                assert!((g.emissive[k] - w.emissive[k]).abs() < 1e-6, "{at}: emissive");
            }
            assert!((g.roughness - w.roughness).abs() < 1e-6, "{at}: roughness {} vs {}", g.roughness, w.roughness);
            assert_eq!(g.metallic, w.metallic, "{at}: metallic");
            assert_eq!(g.alpha_mode, w.alpha_mode, "{at}: alpha mode");
            assert_eq!(g.double_sided, w.double_sided, "{at}: double-sided");
            assert_eq!(g.unlit, w.unlit, "{at}: unlit");
            assert_eq!(g.base_color_texture.as_ref().map(|t| t.uv_set), w.base_color_texture.as_ref().map(|t| t.uv_set), "{at}: base texture");
            assert_eq!(g.emissive_texture.is_some(), w.emissive_texture.is_some(), "{at}: emissive texture");
            if let (Some(gt), Some(wt)) = (&g.base_color_texture, &w.base_color_texture) {
                let gt = &got.textures[gt.texture];
                let wt = &want.textures[wt.texture];
                assert_eq!(got.images[gt.image].data, want.images[wt.image].data, "{at}: image bytes");
                assert_eq!(gt.sampler, wt.sampler, "{at}: sampler");
            }
        }
        assert_eq!(got.skins.len(), want.skins.len(), "{ctx}: skins");
        for (g, w) in got.skins.iter().zip(&want.skins) {
            assert_eq!(g.joints, w.joints, "{ctx}: skin joints");
            assert_eq!(g.inverse_bind_matrices, w.inverse_bind_matrices, "{ctx}: inverse binds (f64 text is exact)");
        }
        assert_eq!(got.animations.len(), want.animations.len(), "{ctx}: animations");
        for (ai, (g, w)) in got.animations.iter().zip(&want.animations).enumerate() {
            assert_eq!(g.name, w.name, "{ctx}: animation {ai} name");
            let (lo, hi) = w.time_range().unwrap();
            assert_eq!(g.time_range(), Some((lo, hi)), "{ctx}: animation {ai} range");
            // Between keys, where interpolation differences show: the
            // resampled matrices must follow the source within a hair.
            for k in 0..=97 {
                let t = lo + (hi - lo) * k as f32 / 97.0;
                let (gp, wp) = (pose::sample_pose(&got, ai, t).locals(), pose::sample_pose(&want, ai, t).locals());
                for (n, (a, b)) in gp.iter().zip(&wp).enumerate() {
                    let d = a.iter().zip(b).map(|(x, y)| (x - y).abs()).fold(0.0, f64::max);
                    assert!(d < 2e-3, "{ctx}: animation {ai} node {n} at t={t} off by {d}");
                }
            }
        }
    }

    #[test]
    fn every_fixture_round_trips_as_a_package() {
        for (name, fixture) in fixtures::all() {
            assert_round_trip(name, &fixture, WriteOptions { self_contained: false });
        }
    }

    #[test]
    fn every_fixture_round_trips_as_a_single_file() {
        for (name, fixture) in fixtures::all() {
            assert_round_trip(name, &fixture, WriteOptions { self_contained: true });
        }
    }

    #[test]
    fn writing_is_deterministic() {
        for (name, fixture) in fixtures::all() {
            let opts = WriteOptions { self_contained: false };
            assert_eq!(package(&fixture, opts), package(&fixture, opts), "{name}");
        }
    }

    #[test]
    fn the_flat_api_round_trips_bit_exactly() {
        let mesh = pose::flatten(&fixtures::cube()).unwrap();
        let back = read(&write(&mesh).unwrap()).unwrap();
        assert_eq!(back.positions, mesh.positions);
        assert_eq!(back.indices, mesh.indices);
        assert_eq!(back.normals, mesh.normals);
        assert_eq!(back.uvs, mesh.uvs);
        let (b, m) = (back.base_color.unwrap(), mesh.base_color.unwrap());
        assert!((0..4).all(|k| (b[k] - m[k]).abs() < 1e-6), "{b:?} vs {m:?}");
    }

    // ── what the writer emits, and why ──────────────────────────────────────

    #[test]
    fn writes_a_collada_141_document_in_metres_y_up() {
        let t = text(&package(&fixtures::cube(), WriteOptions::default()));
        assert!(t.starts_with("<?xml version=\"1.0\" encoding=\"utf-8\"?>\n<COLLADA xmlns=\"http://www.collada.org/2005/11/COLLADASchema\" version=\"1.4.1\">"));
        assert!(t.contains("<unit name=\"meter\" meter=\"1\"/>") && t.contains("<up_axis>Y_UP</up_axis>"));
        assert!(t.contains(&format!("<created>{TIMESTAMP}</created>")));
        // Matrices as Collada writes them: the translation in the 4th column.
        let mut s = fixtures::cube();
        s.nodes[0].transform = Transform::Trs { translation: [1.0, 2.0, 3.0], rotation: math::QUAT_IDENTITY, scale: [1.0; 3] };
        let t = text(&package(&s, WriteOptions::default()));
        assert!(t.contains(">1 0 0 1 0 1 0 2 0 0 1 3 0 0 0 1</matrix>"), "{t}");
    }

    #[test]
    fn material_colours_are_srgb_and_vertex_colours_linear_inside_vertices() {
        let s = fixtures::textured();
        let t = text(&package(&s, WriteOptions { self_contained: false }));
        // wedge_mat's linear 0.9 0.3 0.1 as three.js and SceneKit read it.
        let c = [0.9f32, 0.3, 0.1].map(srgb_encode);
        assert!(t.contains(&format!("<color sid=\"diffuse\">{} {} {} 1</color>", c[0], c[1], c[2])), "diffuse not sRGB-encoded");
        // The panel's colours: a COLOR input inside <vertices>, values as-is.
        let v = t.find("<vertices id=\"panel-mesh-vertices\">").unwrap();
        assert!(t[v..v + 300].contains("semantic=\"COLOR\""));
        let first = s.meshes[2].primitives[0].colors.as_ref().unwrap()[1];
        assert!(t.contains(&floats(&first)), "linear colours written as-is");
        let back = read_package(&package(&s, WriteOptions { self_contained: false })).scene;
        assert_eq!(back.meshes[2].primitives[0].colors, s.meshes[2].primitives[0].colors);
    }

    #[test]
    fn a_single_file_carries_no_images_and_says_so() {
        let w = super::super::write_scene(&fixtures::textured(), MeshFormat::Dae, WriteOptions::default()).unwrap();
        assert_eq!(w.package.files.len(), 1);
        let t = text(&w.package);
        assert!(!t.contains("<library_images>") && !t.contains("<texture "));
        assert!(w.losses.iter().any(|l| l.what.contains("texture binding")), "{:?}", w.losses);
    }

    #[test]
    fn a_package_carries_textures_beside_the_document() {
        let pkg = package(&fixtures::textured(), WriteOptions { self_contained: false });
        assert_eq!(pkg.files[0].path, "model.dae");
        let tex = pkg.files.iter().find(|f| f.path == "textures/quadrants.png").expect("texture sidecar");
        assert_eq!(tex.bytes, fixtures::quadrant_png());
        assert!(text(&pkg).contains("<init_from>textures/quadrants.png</init_from>"));
    }

    #[test]
    fn morph_targets_are_baked_and_the_loss_is_reported() {
        let s = fixtures::morph();
        let w = super::super::write_scene(&s, MeshFormat::Dae, WriteOptions::default()).unwrap();
        let t = text(&w.package);
        assert!(!t.contains("<morph") && !t.contains("<controller"), "no morph controller may be written");
        let losses: Vec<String> = w.losses.iter().map(|l| l.to_string()).collect();
        assert!(losses.iter().any(|l| l.contains("morph targets")), "{losses:?}");
        assert!(losses.iter().any(|l| l.contains("morph-weight animation")), "{losses:?}");
        // The geometry is the default-weight bake: bulge at 0.25.
        let back = read_package(&w.package).scene;
        let want = pose::bake_default(&s).unwrap();
        let (g, b) = (world_positions(&back), world_positions(&want));
        assert_eq!(g.len(), b.len());
        assert!(g.iter().zip(&b).all(|(x, y)| close3(*x, *y, 1e-5)));
    }

    #[test]
    fn step_keys_get_a_key_just_before_them() {
        // The skinned fixture's hip translation steps from 0 to 0.4 at t=1.
        let back = read_package(&package(&fixtures::skinned(), WriteOptions::default())).scene;
        let hip = |t: f32| pose::sample_pose(&back, 0, t).locals()[0][12];
        assert!((hip(0.9998) - 0.0).abs() < 1e-3, "{}", hip(0.9998));
        assert!((hip(1.0) - 0.4).abs() < 1e-6, "{}", hip(1.0));
        assert!((hip(0.5) - 0.0).abs() < 1e-6 && (hip(1.5) - 0.4).abs() < 1e-6);
    }

    #[test]
    fn later_skinned_primitives_are_drawn_from_child_nodes() {
        // SceneKit keeps one instance_controller per node.
        let mut s = fixtures::skinned();
        let p = s.meshes[0].primitives[0].clone();
        s.meshes[0].primitives.push(p);
        let t = text(&package(&s, WriteOptions::default()));
        let node = t.find("<node id=\"tube_node\"").unwrap();
        let first_close = t[node..].find("</node>").unwrap() + node;
        assert_eq!(t[node..first_close].matches("<instance_controller").count(), 2, "one on the node, one on its child");
        assert!(t[node..first_close].contains("<node id=\"tube_node-skin1\""));
        // Both read back as one skin.
        let back = read_package(&package(&s, WriteOptions::default())).scene;
        assert_eq!(back.skins.len(), 1);
    }

    #[test]
    fn a_joint_holding_a_mesh_draws_it_from_a_child() {
        let mut s = fixtures::skinned();
        s.nodes[1].mesh = Some(0);
        s.nodes[1].skin = None;
        s.meshes.push(s.meshes[0].clone());
        s.meshes[1].primitives[0].skin = None;
        s.nodes[1].mesh = Some(1);
        let t = text(&package(&s, WriteOptions::default()));
        let knee = t.find("<node id=\"knee\"").unwrap();
        let geom = t.find("<node id=\"knee-geometry\"").unwrap();
        let inst = t[knee..].find("<instance_geometry").unwrap() + knee;
        assert!(geom > knee && inst > geom, "the instance sits in the child, not on the joint");
    }

    #[test]
    fn ids_are_unique_ncnames_whatever_the_names() {
        let mut s = fixtures::textured();
        for (i, n) in s.nodes.iter_mut().enumerate() {
            n.name = Some(["1 box", "box", "box", "<&>\"'", "ünï\n\tcødé"][i].to_string());
        }
        s.materials[0].name = Some("box".into());
        let pkg = package(&s, WriteOptions { self_contained: false });
        let t = text(&pkg);
        let root = super::super::xml::parse(&pkg.files[0].bytes).unwrap();
        let mut ids = HashSet::new();
        let mut stack = vec![&root];
        while let Some(e) = stack.pop() {
            if let Some(id) = e.attr("id") {
                assert!(ids.insert(id.to_string()), "duplicate id {id}");
                assert!(id.starts_with(|c: char| c.is_ascii_alphabetic() || c == '_'), "{id}");
                assert!(id.chars().all(|c| c.is_ascii_alphanumeric() || "_-.".contains(c)), "{id}");
            }
            stack.extend(e.children.iter());
        }
        // Names are NCNames too (1.4.1's type for `name`; SceneKit cuts a
        // name at its first space): letters of any script survive, the rest
        // become `_`.
        let names: Vec<String> = root
            .descendants_named("node")
            .iter()
            .filter_map(|n| n.attr("name").map(str::to_string))
            .collect();
        for want in ["_1_box", "box", "_____", "ünï__cødé"] {
            assert!(names.iter().any(|n| n == want), "{want} not among {names:?}");
        }
        assert!(!t.contains("name=\"1 box\""));
        let back = read_package(&pkg).scene;
        assert_eq!(back.nodes[4].name.as_deref(), Some("ünï__cødé"));
    }

    // ── reader: geometry ────────────────────────────────────────────────────

    #[test]
    fn reads_polylists_ear_clipping_concave_polygons() {
        // A quad and a dart whose reflex corner is (1,1): fanned from its
        // first corner it would cover the notch and flip a triangle.
        let pos = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0, 3.0, 0.0, 0.0, 5.0, 1.0, 0.0, 3.0, 2.0, 0.0, 4.0, 1.0, 0.0];
        let body = scene_doc(
            &geom("g", &pos, r##"<polylist count="2"><input semantic="VERTEX" source="#g-v" offset="0"/><vcount>4 4</vcount><p>0 1 2 3 4 5 6 7</p></polylist>"##),
            r##"<node id="n"><instance_geometry url="#g"/></node>"##,
        );
        let s = read_doc(&body).unwrap().scene;
        let areas = signed_area_z(&s);
        assert_eq!(areas.len(), 4);
        assert!(areas.iter().all(|a| *a > 0.0), "every triangle keeps the polygon's winding: {areas:?}");
        let total: f32 = areas.iter().sum();
        assert!((total - 2.0).abs() < 1e-5, "quad 1 + dart 1, no overlap: {total}");
    }

    #[test]
    fn reads_polygons_fans_and_strips_with_their_winding() {
        let pos = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0];
        for (kind, p) in [("polygons", "<p>0 1 2 3</p>"), ("trifans", "<p>0 1 2 3</p>"), ("tristrips", "<p>0 1 3 2</p>")] {
            let body = scene_doc(
                &geom("g", &pos, &format!(r##"<{kind} count="1"><input semantic="VERTEX" source="#g-v" offset="0"/>{p}</{kind}>"##)),
                r##"<node id="n"><instance_geometry url="#g"/></node>"##,
            );
            let s = read_doc(&body).unwrap_or_else(|e| panic!("{kind}: {e}")).scene;
            let areas = signed_area_z(&s);
            assert_eq!(areas.len(), 2, "{kind}");
            assert!(areas.iter().all(|a| (*a - 0.5).abs() < 1e-6), "{kind}: {areas:?}");
        }
    }

    #[test]
    fn skips_lines_with_a_warning_and_refuses_holes() {
        let body = scene_doc(
            &geom("g", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles><lines count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1</p></lines>"##),
            r##"<node id="n"><instance_geometry url="#g"/></node>"##,
        );
        let r = read_doc(&body).unwrap();
        assert_eq!(r.scene.triangle_count(), 1);
        assert!(r.warnings.iter().any(|w| w.contains("lines")), "{:?}", r.warnings);
        let holes = scene_doc(
            &geom("g", &TRI, r##"<polygons count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><ph><p>0 1 2</p><h>0 1 2</h></ph></polygons>"##),
            r##"<node id="n"><instance_geometry url="#g"/></node>"##,
        );
        assert!(matches!(read_doc(&holes), Err(MeshError::Unsupported(_))));
    }

    #[test]
    fn reads_separate_index_streams_and_welds_identical_corners() {
        // Two triangles sharing an edge, with per-face normals at their own
        // offset: the shared corners split, identical tuples weld.
        let pos = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 1.0, 1.0, 0.0, 0.0, 1.0, 0.0];
        let prims = r##"<triangles count="2"><input semantic="VERTEX" source="#g-v" offset="0"/><input semantic="NORMAL" source="#n" offset="1"/><p>0 0 1 0 2 0 0 1 2 1 3 1</p></triangles>"##;
        let normals = r##"<source id="n"><float_array id="na" count="6">0 0 1 0 0.6 0.8</float_array><technique_common><accessor source="#na" count="2" stride="3"><param name="X" type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source>"##;
        let g = geom("g", &pos, prims).replacen("<vertices", &format!("{normals}<vertices"), 1);
        let s = read_doc(&scene_doc(&g, r##"<node id="n0"><instance_geometry url="#g"/></node>"##)).unwrap().scene;
        let p = &s.meshes[0].primitives[0];
        assert_eq!(p.positions.len(), 6, "vertices 0 and 2 each appear with two normals");
        assert_eq!(p.indices.len(), 6);
        let n = p.normals.as_ref().unwrap();
        assert_eq!(n[p.indices[3] as usize], [0.0, 0.6, 0.8]);
    }

    #[test]
    fn honours_accessor_offset_stride_and_unnamed_params() {
        // Element i at 1 + 4i: X, an unnamed (skipped) value, then Y and Z.
        let src = r##"<source id="g-p"><float_array id="g-pa" count="13">99 0 7 0 0 1 7 0 0 0 7 1 0</float_array><technique_common><accessor source="#g-pa" count="3" stride="4" offset="1"><param name="X" type="float"/><param type="float"/><param name="Y" type="float"/><param name="Z" type="float"/></accessor></technique_common></source>"##;
        let g = format!(
            r##"<geometry id="g"><mesh>{src}<vertices id="g-v"><input semantic="POSITION" source="#g-p"/></vertices><triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles></mesh></geometry>"##
        );
        let s = read_doc(&scene_doc(&g, r##"<node id="n"><instance_geometry url="#g"/></node>"##)).unwrap().scene;
        assert_eq!(s.meshes[0].primitives[0].positions, vec![[0.0, 0.0, 0.0], [1.0, 0.0, 0.0], [0.0, 1.0, 0.0]]);
    }

    #[test]
    fn texture_coordinates_flip_to_v_down_and_follow_their_set_numbers() {
        // One set, numbered 1 (SceneKit's spelling), plus a second numbered 3.
        let uv = |id: &str, v: &str| format!(r##"<source id="{id}"><float_array id="{id}a" count="6">{v}</float_array><technique_common><accessor source="#{id}a" count="3" stride="2"><param name="S" type="float"/><param name="T" type="float"/></accessor></technique_common></source>"##);
        let prims = r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><input semantic="TEXCOORD" source="#t3" offset="0" set="3"/><input semantic="TEXCOORD" source="#t1" offset="0" set="1"/><p>0 1 2</p></triangles>"##;
        let g = geom("g", &TRI, prims).replacen("<vertices", &format!("{}{}<vertices", uv("t1", "0 0 1 0 0 1"), uv("t3", "0.25 0.25 0.5 0.5 0.75 0.75")), 1);
        let s = read_doc(&scene_doc(&g, r##"<node id="n"><instance_geometry url="#g"/></node>"##)).unwrap().scene;
        let p = &s.meshes[0].primitives[0];
        assert_eq!(p.uvs.len(), 2);
        assert_eq!(p.uvs[0], vec![[0.0, 1.0], [1.0, 1.0], [0.0, 0.0]], "set 1 first, V flipped");
        assert_eq!(p.uvs[1][1], [0.5, 0.5]);
    }

    // ── reader: scene graph ─────────────────────────────────────────────────

    #[test]
    fn converts_units_and_up_axes_at_the_root() {
        let cases = [
            ("Z_UP", "0.01", [0.01f32, 0.03, -0.02]),
            ("X_UP", "1", [-2.0, 1.0, 3.0]),
            ("Y_UP", "2", [2.0, 4.0, 6.0]),
        ];
        for (up, meter, want) in cases {
            let body = format!(
                r#"<asset><unit meter="{meter}"/><up_axis>{up}</up_axis></asset>{}"#,
                scene_doc(&geom("g", &[1.0, 2.0, 3.0, 0.0, 0.0, 0.0, 0.0, 0.0, 1.0], r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles>"##), r##"<node id="n"><instance_geometry url="#g"/></node>"##)
            );
            // A second <asset> would be invalid; build the document by hand.
            let bytes = format!(r#"<?xml version="1.0"?><COLLADA xmlns="http://www.collada.org/2005/11/COLLADASchema" version="1.4.1">{body}</COLLADA>"#);
            let s = read_scene(bytes.as_bytes(), &NoSidecars).unwrap().scene;
            let p = world_positions(&s)[0];
            assert!(close3(p, want, 1e-6), "{up} {meter}: {p:?}");
        }
    }

    #[test]
    fn composes_transform_elements_in_document_order() {
        // (1,0,0) scaled by 2, turned 90° about Z, then moved by +x.
        let s = read_doc(&tri_doc(r#"<translate>1 0 0</translate><rotate>0 0 1 90</rotate><scale>2 2 2</scale>"#)).unwrap().scene;
        assert!(close3(world_positions(&s)[1], [1.0, 2.0, 0.0], 1e-6), "{:?}", world_positions(&s));
        // A row-major <matrix> followed by a translate.
        let s = read_doc(&tri_doc(r#"<matrix>1 0 0 5 0 1 0 0 0 0 1 0 0 0 0 1</matrix><translate>0 1 0</translate>"#)).unwrap().scene;
        assert!(close3(world_positions(&s)[0], [5.0, 1.0, 0.0], 1e-6));
        // <lookat>: from (0,0,5) toward the origin, up +Y: identity rotation.
        let s = read_doc(&tri_doc(r#"<lookat>0 0 5 0 0 0 0 1 0</lookat>"#)).unwrap().scene;
        assert!(close3(world_positions(&s)[1], [1.0, 0.0, 5.0], 1e-6));
        assert!(matches!(read_doc(&tri_doc(r#"<skew>45 1 0 0 0 1 0</skew>"#)), Err(MeshError::Unsupported(_))));
        assert!(matches!(read_doc(&tri_doc(r#"<rotate>0 0 0 30</rotate>"#)), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn instance_node_copies_library_nodes_and_keeps_meshes_shared() {
        let g = geom("g", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles>"##);
        let body = format!(
            r##"<library_geometries>{g}</library_geometries><library_nodes><node id="part"><instance_geometry url="#g"/></node></library_nodes><library_visual_scenes><visual_scene id="vs"><node id="a"><translate>5 0 0</translate><instance_node url="#part"/></node><node id="b"><instance_node url="#part"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##
        );
        let s = read_doc(&body).unwrap().scene;
        assert_eq!(s.nodes.len(), 4);
        assert_eq!(s.meshes.len(), 1, "one geometry, instanced twice");
        let p = world_positions(&s);
        assert!(close3(p[1], [6.0, 0.0, 0.0], 1e-6) && close3(p[4], [1.0, 0.0, 0.0], 1e-6), "{p:?}");
    }

    #[test]
    fn an_instance_node_cycle_is_refused() {
        let body = r##"<library_nodes><node id="a"><instance_node url="#b"/></node><node id="b"><instance_node url="#a"/></node></library_nodes><library_visual_scenes><visual_scene id="vs"><node id="root"><instance_node url="#a"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##;
        assert!(matches!(read_doc(body), Err(MeshError::Malformed(_))));
        let selfref = r##"<library_visual_scenes><visual_scene id="vs"><node id="a"><instance_node url="#a"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##;
        assert!(matches!(read_doc(selfref), Err(MeshError::Malformed(_))));
    }

    #[test]
    fn an_instance_node_bomb_is_refused_quickly() {
        // Each level instances the next twice: 2^40 nodes from a 3 KB file.
        let mut lib = String::new();
        for i in 0..40 {
            lib.push_str(&format!(r##"<node id="n{i}"><instance_node url="#n{}"/><instance_node url="#n{}"/></node>"##, i + 1, i + 1));
        }
        lib.push_str(r#"<node id="n40"/>"#);
        let body = format!(r##"<library_nodes>{lib}</library_nodes><library_visual_scenes><visual_scene id="vs"><node id="r"><instance_node url="#n0"/></node></visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##);
        let started = std::time::Instant::now();
        assert!(matches!(read_doc(&body), Err(MeshError::Unsupported(_))));
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "{:?}", started.elapsed());
    }

    #[test]
    fn deep_nesting_is_bounded() {
        let open: String = (0..600).map(|i| format!(r#"<node id="d{i}">"#)).collect();
        let close = "</node>".repeat(600);
        let body = format!(r##"<library_visual_scenes><visual_scene id="vs">{open}{close}</visual_scene></library_visual_scenes><scene><instance_visual_scene url="#vs"/></scene>"##);
        assert!(read_doc(&body).is_err());
    }

    // ── reader: materials and images ────────────────────────────────────────

    fn material_doc(effect: &str, newparams: &str, bind: &str, prim_inputs: &str) -> String {
        let g = geom(
            "g",
            &TRI,
            &format!(r##"<triangles count="1" material="sym"><input semantic="VERTEX" source="#g-v" offset="0"/>{prim_inputs}<p>0 1 2</p></triangles>"##),
        )
        .replacen(
            "<vertices",
            r##"<source id="uv"><float_array id="uva" count="6">0 0 1 0 0 1</float_array><technique_common><accessor source="#uva" count="3" stride="2"><param name="S" type="float"/><param name="T" type="float"/></accessor></technique_common></source><vertices"##,
            1,
        );
        format!(
            r##"<library_images><image id="img"><init_from>tex/a.png</init_from></image></library_images><library_effects><effect id="fx"><profile_COMMON>{newparams}<technique sid="common">{effect}</technique></profile_COMMON></effect></library_effects><library_materials><material id="m" name="mat"><instance_effect url="#fx"/></material></library_materials>{}"##,
            scene_doc(&g, &format!(r##"<node id="n"><instance_geometry url="#g"><bind_material><technique_common><instance_material symbol="sym" target="#m">{bind}</instance_material></technique_common></bind_material></instance_geometry></node>"##))
        )
    }

    const SAMPLER: &str = r#"<newparam sid="surf"><surface type="2D"><init_from>img</init_from></surface></newparam><newparam sid="samp"><sampler2D><source>surf</source><wrap_s>CLAMP</wrap_s><wrap_t>MIRROR</wrap_t></sampler2D></newparam>"#;
    const UV0: &str = r##"<input semantic="TEXCOORD" source="#uv" offset="0" set="0"/>"##;

    fn with_png(body: &str) -> Result<SceneRead, MeshError> {
        let files = vec![PackageFile { path: "tex/a.png".into(), bytes: fixtures::quadrant_png() }];
        read_scene(&doc(body), &MemorySidecars { files: &files })
    }

    #[test]
    fn resolves_textures_through_the_sampler_and_surface() {
        let body = material_doc(
            r#"<phong><diffuse><texture texture="samp" texcoord="CH0"/></diffuse><shininess><float>30</float></shininess></phong>"#,
            SAMPLER,
            r#"<bind_vertex_input semantic="CH0" input_semantic="TEXCOORD" input_set="0"/>"#,
            UV0,
        );
        let s = with_png(&body).unwrap().scene;
        let m = &s.materials[0];
        assert_eq!(m.name.as_deref(), Some("mat"));
        let r = m.base_color_texture.as_ref().unwrap();
        assert_eq!(s.images[s.textures[r.texture].image].data, fixtures::quadrant_png());
        assert_eq!(s.textures[r.texture].sampler.wrap_s, Wrap::ClampToEdge);
        assert_eq!(s.textures[r.texture].sampler.wrap_t, Wrap::MirroredRepeat);
        assert!((m.roughness - roughness_from_shininess(30.0)).abs() < 1e-6);
        // Without the file, the texture is dropped and that is said.
        let r = read_doc(&body).unwrap();
        assert!(r.scene.materials[0].base_color_texture.is_none());
        assert!(r.warnings.iter().any(|w| w.contains("not supplied")), "{:?}", r.warnings);
    }

    #[test]
    fn a_texture_naming_an_image_directly_is_used_with_a_warning() {
        let body = material_doc(r#"<lambert><diffuse><texture texture="img" texcoord="x"/></diffuse></lambert>"#, "", "", UV0);
        let r = with_png(&body).unwrap();
        assert!(r.scene.materials[0].base_color_texture.is_some());
        assert!(r.warnings.iter().any(|w| w.contains("directly")), "{:?}", r.warnings);
    }

    #[test]
    fn absolute_image_paths_resolve_by_file_name_and_hex_images_decode() {
        let body = material_doc(r#"<lambert><diffuse><texture texture="samp" texcoord="x"/></diffuse></lambert>"#, SAMPLER, "", UV0)
            .replace("<init_from>tex/a.png</init_from>", "<init_from>file:///C:/work/a.png</init_from>");
        let files = vec![PackageFile { path: "a.png".into(), bytes: fixtures::quadrant_png() }];
        let r = read_scene(&doc(&body), &MemorySidecars { files: &files }).unwrap();
        assert!(r.scene.materials[0].base_color_texture.is_some());
        assert!(r.warnings.iter().any(|w| w.contains("beside the document")), "{:?}", r.warnings);
        let hex: String = fixtures::quadrant_png().iter().map(|b| format!("{b:02X}")).collect();
        let body = material_doc(r#"<lambert><diffuse><texture texture="samp" texcoord="x"/></diffuse></lambert>"#, SAMPLER, "", UV0)
            .replace("<init_from>tex/a.png</init_from>", &format!("<data>{hex}</data>"));
        let s = read_doc(&body).unwrap().scene;
        assert_eq!(s.images[0].data, fixtures::quadrant_png());
    }

    #[test]
    fn reads_colours_as_srgb_and_every_transparency_mode() {
        let c = srgb_encode(0.25);
        let cases: Vec<(String, AlphaMode, f32)> = vec![
            (format!(r#"<lambert><diffuse><color>{c} {c} {c} 1</color></diffuse></lambert>"#), AlphaMode::Opaque, 1.0),
            (r#"<lambert><transparent opaque="A_ONE"><color>1 1 1 0.4</color></transparent><transparency><float>1</float></transparency></lambert>"#.into(), AlphaMode::Blend, 0.4),
            (r#"<lambert><transparent opaque="RGB_ZERO"><color>0.25 0.25 0.25 1</color></transparent><transparency><float>1</float></transparency></lambert>"#.into(), AlphaMode::Blend, 0.75),
            (r#"<lambert><transparency><float>0.5</float></transparency></lambert>"#.into(), AlphaMode::Blend, 0.5),
            (r#"<lambert><transparent opaque="A_ONE"><color>1 1 1 1</color></transparent><transparency><float>1</float></transparency></lambert>"#.into(), AlphaMode::Opaque, 1.0),
        ];
        for (effect, mode, alpha) in cases {
            let s = read_doc(&material_doc(&effect, "", "", "")).unwrap().scene;
            let m = &s.materials[0];
            assert_eq!(m.alpha_mode, mode, "{effect}");
            assert!((m.base_color[3] - alpha).abs() < 1e-6, "{effect}: {:?}", m.base_color);
        }
        let s = read_doc(&material_doc(&format!(r#"<lambert><diffuse><color>{c} {c} {c} 1</color></diffuse></lambert>"#), "", "", "")).unwrap().scene;
        assert!((s.materials[0].base_color[0] - 0.25).abs() < 1e-6);
    }

    #[test]
    fn reads_constant_shading_param_refs_and_double_sided_extras() {
        let s = read_doc(&material_doc(r#"<constant><emission><color>1 0 0 1</color></emission></constant>"#, "", "", "")).unwrap().scene;
        assert!(s.materials[0].unlit && s.materials[0].base_color[..3] == [1.0, 0.0, 0.0]);
        let np = r#"<newparam sid="tint"><float4>0 1 0 1</float4></newparam>"#;
        let fx = r#"<lambert><diffuse><param ref="tint"/></diffuse></lambert><extra><technique profile="MAX3D"><double_sided>1</double_sided></technique></extra>"#;
        let s = read_doc(&material_doc(fx, np, "", "")).unwrap().scene;
        assert_eq!(s.materials[0].base_color[..3], [0.0, 1.0, 0.0]);
        assert!(s.materials[0].double_sided);
    }

    // ── reader: controllers ─────────────────────────────────────────────────

    /// A two-vertex-weighted triangle skinned to joints `j0`/`j1`.
    fn skin_doc(bind_shape: &str, joints: &str, v: &str, vcount: &str, weights: &str) -> String {
        let g = geom("g", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#g-v" offset="0"/><p>0 1 2</p></triangles>"##);
        format!(
            r##"<library_controllers><controller id="c"><skin source="#g">{bind_shape}<source id="cj">{joints}<technique_common><accessor source="#cja" count="2" stride="1"><param name="JOINT" type="name"/></accessor></technique_common></source><source id="cm"><float_array id="cma" count="32">1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1 1 0 0 0 0 1 0 -1 0 0 1 0 0 0 0 1</float_array><technique_common><accessor source="#cma" count="2" stride="16"><param name="TRANSFORM" type="float4x4"/></accessor></technique_common></source><source id="cw"><float_array id="cwa" count="{}">{weights}</float_array><technique_common><accessor source="#cwa" count="{}" stride="1"><param name="WEIGHT" type="float"/></accessor></technique_common></source><joints><input semantic="JOINT" source="#cj"/><input semantic="INV_BIND_MATRIX" source="#cm"/></joints><vertex_weights count="3"><input semantic="JOINT" source="#cj" offset="0"/><input semantic="WEIGHT" source="#cw" offset="1"/><vcount>{vcount}</vcount><v>{v}</v></vertex_weights></skin></controller></library_controllers>{}"##,
            weights.split_whitespace().count(),
            weights.split_whitespace().count(),
            scene_doc(&g, r##"<node id="j0" name="j0" sid="J0" type="JOINT"><node id="j1" name="j1" sid="J1" type="JOINT"><translate>0 1 0</translate></node></node><node id="m"><translate>9 9 9</translate><instance_controller url="#c"><skeleton>#j0</skeleton></instance_controller></node>"##)
        )
    }

    #[test]
    fn skins_resolve_joints_by_sid_and_fold_the_bind_shape_matrix() {
        let body = skin_doc(
            "<bind_shape_matrix>1 0 0 0 0 1 0 2 0 0 1 0 0 0 0 1</bind_shape_matrix>",
            r#"<Name_array id="cja" count="2">J0 J1</Name_array>"#,
            "0 0 1 0 1 0",
            "1 1 1",
            "1",
        );
        let s = read_doc(&body).unwrap().scene;
        assert_eq!(s.skins.len(), 1);
        let sk = &s.skins[0];
        assert_eq!(sk.joints.iter().map(|j| s.nodes[*j].name.clone().unwrap()).collect::<Vec<_>>(), ["j0", "j1"]);
        // IBM(j1) = T(0,-1,0), folded with the bind shape T(0,2,0): T(0,1,0).
        assert_eq!(sk.inverse_bind_matrices[1], math::translation([0.0, 1.0, 0.0]));
        // Vertex 0 rides j0 (at the origin) with the bind shape: (0,2,0).
        // Vertex 1 rides j1: JM·IBM·BSM·v = T(0,1,0)·T(0,-1,0)·T(0,2,0)·(1,0,0).
        // The instancing node's own (9,9,9) is ignored, as the scene requires.
        let p = world_positions(&s);
        assert!(close3(p[0], [0.0, 2.0, 0.0], 1e-6) && close3(p[1], [1.0, 2.0, 0.0], 1e-6), "{p:?}");
    }

    #[test]
    fn skins_take_idref_joints_and_the_bind_shape_index() {
        let body = skin_doc("", r#"<IDREF_array id="cja" count="2">j0 j1</IDREF_array>"#, "0 0 -1 0 1 0", "1 1 1", "1")
            .replace(r#"<param name="JOINT" type="name"/>"#, r#"<param name="JOINT" type="IDREF"/>"#);
        let s = read_doc(&body).unwrap().scene;
        // -1 binds vertex 1 to the bind shape: the instancing node, which
        // becomes a third joint.
        assert_eq!(s.skins[0].joints.len(), 3);
        assert!(close3(world_positions(&s)[1], [10.0, 9.0, 9.0], 1e-6), "{:?}", world_positions(&s));
    }

    #[test]
    fn a_skin_of_a_skin_is_refused_without_recursing() {
        // A skin naming itself, and a 100k-long chain of skins: each must be
        // refused at the first link, not resolved (which recursed forever, or
        // as deep as the chain).
        let ctrl = |id: &str, src: &str| format!(r##"<controller id="{id}"><skin source="#{src}"><joints/><vertex_weights count="0"/></skin></controller>"##);
        let scene = |c: &str| scene_doc(&geom("g", &TRI, ""), &format!(r##"<node id="n"><instance_controller url="#{c}"/></node>"##));
        let body = format!("<library_controllers>{}</library_controllers>{}", ctrl("s", "s"), scene("s"));
        assert!(matches!(read_doc(&body), Err(MeshError::Unsupported(_))));
        let chain: String = (0..100_000).map(|i| ctrl(&format!("s{i}"), &format!("s{}", i + 1))).collect();
        let body = format!("<library_controllers>{chain}</library_controllers>{}", scene("s0"));
        assert!(matches!(read_doc(&body), Err(MeshError::Unsupported(_))));
    }

    #[test]
    fn skin_weights_that_disagree_with_their_counts_are_refused() {
        for (v, vcount) in [("0 0 1 0", "1 1 1"), ("0 0 1 0 5 0", "1 1 1"), ("0 0 1 0 1 7", "1 1 1"), ("0 0 1 0 1 0", "1 1")] {
            let body = skin_doc("", r#"<Name_array id="cja" count="2">J0 J1</Name_array>"#, v, vcount, "1");
            assert!(matches!(read_doc(&body), Err(MeshError::Malformed(_))), "{v} / {vcount}");
        }
    }

    fn morph_doc(method: &str, target: &[f32], weight: &str, anim: &str) -> String {
        let base = geom("base", &TRI, r##"<triangles count="1"><input semantic="VERTEX" source="#base-v" offset="0"/><p>0 1 2</p></triangles>"##);
        let tgt = geom("tgt", target, "");
        format!(
            r##"<library_controllers><controller id="mc"><morph source="#base" method="{method}"><source id="mt"><IDREF_array id="mta" count="1">tgt</IDREF_array><technique_common><accessor source="#mta" count="1" stride="1"><param name="IDREF" type="IDREF"/></accessor></technique_common></source><source id="mw"><float_array id="mwa" count="1">{weight}</float_array><technique_common><accessor source="#mwa" count="1" stride="1"><param name="MORPH_WEIGHT" type="float"/></accessor></technique_common></source><targets><input semantic="MORPH_TARGET" source="#mt"/><input semantic="MORPH_WEIGHT" source="#mw"/></targets></morph></controller></library_controllers>{anim}{}"##,
            scene_doc(&format!("{base}{tgt}"), r##"<node id="n"><instance_controller url="#mc"/></node>"##)
        )
    }

    #[test]
    fn morphs_read_normalized_and_relative_with_weight_animation() {
        let anim = r##"<library_animations><animation id="a"><source id="ai"><float_array id="aia" count="2">0 1</float_array><technique_common><accessor source="#aia" count="2" stride="1"><param name="TIME" type="float"/></accessor></technique_common></source><source id="ao"><float_array id="aoa" count="2">0 1</float_array><technique_common><accessor source="#aoa" count="2" stride="1"><param name="WEIGHT" type="float"/></accessor></technique_common></source><sampler id="as"><input semantic="INPUT" source="#ai"/><input semantic="OUTPUT" source="#ao"/></sampler><channel source="#as" target="mwa(0)"/></animation></library_animations>"##;
        let moved = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 3.0, 0.0];
        let s = read_doc(&morph_doc("NORMALIZED", &moved, "0.5", anim)).unwrap().scene;
        let m = &s.meshes[0];
        assert_eq!(m.weights, vec![0.5]);
        assert_eq!(m.primitives[0].targets[0].positions, vec![[0.0; 3], [0.0; 3], [0.0, 2.0, 0.0]], "absolute target minus base");
        assert!(close3(world_positions(&s)[2], [0.0, 2.0, 0.0], 1e-6));
        let w = s.animations[0].channels.iter().find(|c| c.path == AnimPath::Weights).expect("weight channel");
        assert_eq!(w.values, vec![0.0, 1.0]);
        let delta = [0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 0.0, 2.0, 0.0];
        let s = read_doc(&morph_doc("RELATIVE", &delta, "1", "")).unwrap().scene;
        assert_eq!(s.meshes[0].primitives[0].targets[0].positions[2], [0.0, 2.0, 0.0], "relative targets are offsets");
        assert!(close3(world_positions(&s)[2], [0.0, 3.0, 0.0], 1e-6));
        let short = [0.0, 0.0, 0.0, 1.0, 0.0, 0.0];
        assert!(matches!(read_doc(&morph_doc("RELATIVE", &short, "1", "")), Err(MeshError::Malformed(_))));
    }

    // ── reader: animation ───────────────────────────────────────────────────

    fn anim(target: &str, times: &str, values: &str, stride: usize, interp: Option<&str>, tangents: &str) -> String {
        let n = times.split_whitespace().count();
        let params: String = (0..stride).map(|i| format!(r#"<param name="V{i}" type="float"/>"#)).collect();
        let interp_src = interp.map(|i| format!(r##"<source id="ii"><Name_array id="iia" count="{n}">{}</Name_array><technique_common><accessor source="#iia" count="{n}" stride="1"><param name="INTERPOLATION" type="name"/></accessor></technique_common></source>"##, vec![i; n].join(" ")));
        format!(
            r##"<library_animations><animation id="an"><source id="ti"><float_array id="tia" count="{n}">{times}</float_array><technique_common><accessor source="#tia" count="{n}" stride="1"><param name="TIME" type="float"/></accessor></technique_common></source><source id="vo"><float_array id="voa" count="{}">{values}</float_array><technique_common><accessor source="#voa" count="{n}" stride="{stride}">{params}</accessor></technique_common></source>{}{tangents}<sampler id="sm"><input semantic="INPUT" source="#ti"/><input semantic="OUTPUT" source="#vo"/>{}{}</sampler><channel source="#sm" target="{target}"/></animation></library_animations>"##,
            values.split_whitespace().count(),
            interp_src.clone().unwrap_or_default(),
            if interp_src.is_some() { r##"<input semantic="INTERPOLATION" source="#ii"/>"## } else { "" },
            if tangents.is_empty() { "" } else { r##"<input semantic="IN_TANGENT" source="#tin"/><input semantic="OUT_TANGENT" source="#tout"/>"## },
        )
    }

    fn animated(node_content: &str, animation: &str) -> Scene {
        read_doc(&format!("{animation}{}", tri_doc(node_content))).unwrap().scene
    }

    fn x_at(s: &Scene, t: f32) -> f64 {
        pose::sample_pose(s, 0, t).locals()[0][12]
    }

    #[test]
    fn animates_members_elements_and_whole_transforms() {
        let s = animated(r#"<translate sid="loc">0 0 0</translate>"#, &anim("n/loc.X", "0 2", "0 4", 1, Some("LINEAR"), ""));
        assert!((x_at(&s, 1.0) - 2.0).abs() < 1e-5, "{}", x_at(&s, 1.0));
        let s = animated(r#"<matrix sid="m">1 0 0 0 0 1 0 0 0 0 1 0 0 0 0 1</matrix>"#, &anim("n/m(0)(3)", "0 2", "0 4", 1, None, ""));
        assert!((x_at(&s, 0.5) - 1.0).abs() < 1e-5);
        let s = animated(r#"<translate sid="t">0 0 0</translate>"#, &anim("n/t", "0 1", "0 0 0 3 0 0", 3, Some("STEP"), ""));
        assert!((x_at(&s, 0.999) - 0.0).abs() < 1e-9 && (x_at(&s, 1.0) - 3.0).abs() < 1e-9);
        assert_eq!(s.animations[0].channels[0].interpolation, Interpolation::Step);
        // rotate ANGLE: 0 → 180° about Z resampled, so the half-way pose is a
        // quarter turn even though slerp between the end keys is ambiguous.
        let s = animated(r#"<rotate sid="rz">0 0 1 0</rotate>"#, &anim("n/rz.ANGLE", "0 1", "0 180", 1, Some("LINEAR"), ""));
        let m = pose::sample_pose(&s, 0, 0.5).locals()[0];
        assert!((m[0] - 0.0).abs() < 1e-3 && (m[1] - 1.0).abs() < 1e-3, "{m:?}");
    }

    #[test]
    fn a_node_scaled_in_from_nothing_reads_and_writes() {
        // Read: scale 0 → 2 on a node; the zero key has no rotation to
        // decompose, which must not make the file unreadable.
        let s = animated(r#"<rotate sid="r">0 0 1 30</rotate><scale sid="s">1 1 1</scale>"#, &anim("n/s", "0 1", "0 0 0 2 2 2", 3, Some("LINEAR"), ""));
        let half = pose::sample_pose(&s, 0, 0.5).trs[0].unwrap();
        assert!(half.2.iter().all(|v| (v - 1.0).abs() < 1e-5), "{half:?}");
        let r30 = math::quat_from_axis_angle([0.0, 0.0, 1.0], 30f64.to_radians());
        assert!(math::quat_dot(half.1, r30).abs() > 1.0 - 1e-6, "the zero key keeps the node's rotation: {half:?}");
        // Write: a zero-scale key goes out as a millionth, which three.js can
        // decompose, and comes back as (nearly) nothing.
        let mut fx = fixtures::skinned();
        let ch = &mut fx.animations[0].channels[3];
        ch.interpolation = Interpolation::Linear;
        ch.values = vec![0.0, 0.0, 0.0, 1.25, 1.25, 1.25, 1.0, 1.0, 1.0];
        let t = text(&package(&fx, WriteOptions::default()));
        let hip = t.find("<float_array id=\"hip-anim-output-array\"").unwrap();
        assert!(t[hip..hip + 120].contains(">0.000001 0 0 0 0 0.000001 0 0 0 0 0.000001 0"), "{}", &t[hip..hip + 120]);
        let back = read_package(&package(&fx, WriteOptions::default())).scene;
        let s0 = pose::sample_pose(&back, 0, 0.0).trs[0].unwrap().2;
        assert!(s0.iter().all(|v| v.abs() < 1e-5), "{s0:?}");
    }

    #[test]
    fn evaluates_bezier_and_hermite_keys() {
        // 2-D Bézier tangents with controls on the chord: a straight line.
        let tangents = r##"<source id="tin"><float_array id="tina" count="4">0 0 1.3333334 2.6666667</float_array><technique_common><accessor source="#tina" count="2" stride="2"><param name="X" type="float"/><param name="Y" type="float"/></accessor></technique_common></source><source id="tout"><float_array id="touta" count="4">0.6666667 1.3333334 2 4</float_array><technique_common><accessor source="#touta" count="2" stride="2"><param name="X" type="float"/><param name="Y" type="float"/></accessor></technique_common></source>"##;
        let s = animated(r#"<translate sid="t">0 0 0</translate>"#, &anim("n/t.X", "0 2", "0 4", 1, Some("BEZIER"), tangents));
        for t in [0.3, 1.0, 1.7] {
            assert!((x_at(&s, t) - 2.0 * t as f64).abs() < 1e-3, "t={t}: {}", x_at(&s, t));
        }
        // Ease in/out with flat 1-D tangents: the midpoint is half-way and the
        // quarter point is the smoothstep's 0.15625 of the way.
        let flat = r##"<source id="tin"><float_array id="tina" count="2">0 4</float_array><technique_common><accessor source="#tina" count="2" stride="1"><param name="X" type="float"/></accessor></technique_common></source><source id="tout"><float_array id="touta" count="2">0 4</float_array><technique_common><accessor source="#touta" count="2" stride="1"><param name="X" type="float"/></accessor></technique_common></source>"##;
        let s = animated(r#"<translate sid="t">0 0 0</translate>"#, &anim("n/t.X", "0 2", "0 4", 1, Some("BEZIER"), flat));
        assert!((x_at(&s, 1.0) - 2.0).abs() < 1e-3);
        assert!(x_at(&s, 0.5) < 1.0, "eases in: {}", x_at(&s, 0.5));
        // Hermite: zero tangents are the same smoothstep.
        let zero = flat.replace(">0 4</float_array>", ">0 0</float_array>");
        let s = animated(r#"<translate sid="t">0 0 0</translate>"#, &anim("n/t.X", "0 2", "0 4", 1, Some("HERMITE"), &zero));
        assert!((x_at(&s, 0.5) - 4.0 * 0.15625).abs() < 2e-2, "{}", x_at(&s, 0.5));
        // Unsupported curve types say so.
        let r = read_doc(&format!("{}{}", anim("n/t.X", "0 2", "0 4", 1, Some("BSPLINE"), ""), tri_doc(r#"<translate sid="t">0 0 0</translate>"#)));
        assert!(matches!(r, Err(MeshError::Unsupported(_))));
    }

    #[test]
    fn animation_clips_become_separate_animations() {
        let a = anim("n/t.X", "0 1", "0 1", 1, None, "");
        let b = a.replace(r#"id="an""#, r#"id="an2""#).replace("0 1</float_array>", "0 2</float_array>").replace(r#"id="sm""#, r#"id="sm2""#).replace("#sm\"", "#sm2\"");
        let clips = r##"<library_animation_clips><animation_clip id="walk" name="Walk" start="0" end="1"><instance_animation url="#an"/></animation_clip><animation_clip id="run" end="1"><instance_animation url="#an2"/></animation_clip></library_animation_clips>"##;
        let body = format!("{a}{}{clips}{}", b.replace("<library_animations>", "<library_animations>"), tri_doc(r#"<translate sid="t">0 0 0</translate>"#));
        let s = read_doc(&body).unwrap().scene;
        assert_eq!(s.animations.len(), 2);
        assert_eq!(s.animations[0].name.as_deref(), Some("Walk"));
        assert_eq!(s.animations[1].name.as_deref(), Some("run"));
    }

    #[test]
    fn a_clip_starts_at_zero_even_when_its_first_key_is_later() {
        // three.js plays a clip for `end - start` seconds: a clip spanning
        // its keys (CesiumMan's begin at 1/24 s) would stop short of the last.
        let mut s = fixtures::skinned();
        for ch in s.animations[0].channels.iter_mut() {
            for t in ch.times.iter_mut() {
                *t += 0.5;
            }
        }
        let (_, hi) = s.animations[0].time_range().unwrap();
        let t = text(&package(&s, WriteOptions::default()));
        let clip = t.split("<animation_clip ").nth(1).expect("a clip").split('>').next().unwrap();
        assert!(clip.contains(r#"start="0""#), "{clip}");
        assert!(clip.contains(&format!(r#"end="{hi}""#)), "{clip}");
    }

    #[test]
    fn channels_on_things_the_scene_does_not_model_are_reported() {
        let r = read_doc(&format!("{}{}", anim("cam/fov", "0 1", "40 50", 1, None, ""), tri_doc(""))).unwrap();
        assert!(r.scene.animations.is_empty());
        assert!(r.warnings.iter().any(|w| w.contains("skipped")), "{:?}", r.warnings);
    }

    // ── independent producers ───────────────────────────────────────────────

    fn testdata(bytes: &[u8]) -> SceneRead {
        let files = vec![
            PackageFile { path: "textures/quadrants.png".into(), bytes: fixtures::quadrant_png() },
            PackageFile { path: "textures/bands.png".into(), bytes: fixtures::bands_png() },
            PackageFile { path: "textured_assimp_texture_0001.png".into(), bytes: fixtures::quadrant_png() },
            PackageFile { path: "basecolor.png".into(), bytes: include_bytes!("testdata/dae/research/basecolor.png").to_vec() },
        ];
        read_scene(bytes, &MemorySidecars { files: &files }).unwrap()
    }

    /// The world-space bounds of a scene at time `t` of its first clip.
    fn bounds_at(s: &Scene, t: Option<f32>) -> ([f32; 3], [f32; 3]) {
        let pose = match t {
            Some(t) => pose::sample_pose(s, 0, t),
            None => pose::default_pose(s),
        };
        let mut lo = [f32::INFINITY; 3];
        let mut hi = [f32::NEG_INFINITY; 3];
        for w in pose::world_primitives(s, &pose).unwrap() {
            for p in w.positions {
                for k in 0..3 {
                    lo[k] = lo[k].min(p[k]);
                    hi[k] = hi[k].max(p[k]);
                }
            }
        }
        (lo, hi)
    }

    #[test]
    fn reads_scenekits_export_of_the_skinned_fixture() {
        let r = testdata(include_bytes!("testdata/dae/scenekit/skinned_scenekit.dae"));
        let (s, fx) = (&r.scene, fixtures::skinned());
        assert_eq!(s.triangle_count(), fx.triangle_count());
        assert_eq!(s.skins.len(), 1);
        let names: Vec<_> = s.skins[0].joints.iter().map(|j| s.nodes[*j].name.clone().unwrap()).collect();
        assert_eq!(names, ["hip", "knee", "ankle"], "sids SID_4.. resolve through <skeleton>");
        assert!(s.materials[0].base_color_texture.is_some(), "set=\"1\" bound through CHANNEL2: {:?}", r.warnings);
        assert_eq!(s.animations.len(), 1);
        // The bend, between keys, against the fixture itself.
        for t in fixtures::sample_times(&fx) {
            let (a, b) = (bounds_at(s, Some(t)), bounds_at(&fx, Some(t)));
            assert!(close3(a.0, b.0, 2e-3) && close3(a.1, b.1, 2e-3), "t={t}: {a:?} vs {b:?}");
        }
    }

    #[test]
    fn reads_scenekits_export_of_the_textured_fixture() {
        let r = testdata(include_bytes!("testdata/dae/scenekit/textured_scenekit.dae"));
        let s = &r.scene;
        let fx = fixtures::textured();
        assert_eq!(s.triangle_count(), fx.triangle_count());
        assert_eq!(bounds_at(s, None).0.map(|v| (v * 1e4).round()), bounds_at(&fx, None).0.map(|v| (v * 1e4).round()));
        assert!(s.meshes.iter().flat_map(|m| &m.primitives).any(|p| p.colors.is_some()), "the panel's vertex colours");
        let wedge = s.materials.iter().find(|m| m.name.as_deref() == Some("wedge_mat")).unwrap();
        assert!((0..3).all(|k| (wedge.base_color[k] - [0.9, 0.3, 0.1][k]).abs() < 2e-3), "{:?}", wedge.base_color);
        assert!(s.materials.iter().any(|m| m.base_color_texture.is_some()));
        let cube = testdata(include_bytes!("testdata/dae/scenekit/cube_scenekit.dae")).scene;
        assert_eq!(bounds_at(&cube, None), bounds_at(&fixtures::cube(), None));
    }

    #[test]
    fn reads_assimps_exports() {
        let s = testdata(include_bytes!("testdata/dae/assimp/textured_assimp.dae")).scene;
        let fx = fixtures::textured();
        assert_eq!(s.triangle_count(), fx.triangle_count());
        let (a, b) = (bounds_at(&s, None), bounds_at(&fx, None));
        assert!(close3(a.0, b.0, 1e-4) && close3(a.1, b.1, 1e-4), "{a:?} vs {b:?}");
        assert!(s.materials.iter().any(|m| m.base_color_texture.is_some()));
        let cube = testdata(include_bytes!("testdata/dae/assimp/cube_assimp.dae")).scene;
        assert_eq!(bounds_at(&cube, None), bounds_at(&fixtures::cube(), None));
    }

    #[test]
    fn reads_the_research_references() {
        let r = testdata(include_bytes!("testdata/dae/research/ref.dae"));
        let s = &r.scene;
        assert_eq!(s.triangle_count(), 5);
        assert_eq!(s.materials.len(), 2);
        assert_eq!(s.skins.len(), 1);
        assert!(s.materials.iter().any(|m| m.double_sided), "GOOGLEEARTH double_sided");
        let red = s.materials.iter().find(|m| m.name.as_deref() == Some("RedMat")).unwrap();
        assert_eq!(red.base_color[..3], [1.0, 0.0, 0.0]);
        assert!(s.animations[0].channels.iter().any(|c| c.interpolation == Interpolation::Step), "Bone0's STEP channel");
        let m = testdata(include_bytes!("testdata/dae/research/morph.dae")).scene;
        assert_eq!(m.meshes[0].target_count(), 1, "morph under a skin");
        assert!(m.animations[0].channels.iter().any(|c| c.path == AnimPath::Weights));
        let rel = testdata(include_bytes!("testdata/dae/research/morph_relative_full.dae")).scene;
        assert_eq!(rel.meshes[0].weights, vec![1.0]);
        let top = world_positions(&rel)[4];
        assert!(close3(top, [0.0, 2.0, 0.0], 1e-6), "-0.5 + 0.5 offset: {top:?}");
    }

    // ── hostile input ───────────────────────────────────────────────────────

    #[test]
    fn other_containers_are_not_this_format() {
        let glb = super::super::write(&pose::flatten(&fixtures::cube()).unwrap(), MeshFormat::Glb).unwrap();
        for bytes in [
            glb,
            b"ply\nformat ascii 1.0\n".to_vec(),
            b"solid x\nendsolid x\n".to_vec(),
            b"v 0 0 0\nf 1 1 1\n".to_vec(),
            b"{\"asset\":{\"version\":\"2.0\"}}".to_vec(),
            b"<?xml version=\"1.0\"?><model xmlns=\"http://schemas.microsoft.com/3dmanufacturing/core/2015/02\"/>".to_vec(),
            b"<!DOCTYPE html><html><body>oops</body></html>".to_vec(),
            vec![],
            vec![0xFF, 0xFE, b'<', 0],
        ] {
            assert!(matches!(read(&bytes), Err(MeshError::NotThisFormat { .. })), "{:?}", String::from_utf8_lossy(&bytes[..bytes.len().min(30)]));
        }
    }

    #[test]
    fn never_panics_on_truncation_or_corruption() {
        let pkg = package(&fixtures::skinned(), WriteOptions::default());
        let full = &pkg.files[0].bytes;
        let step = (full.len() / 400).max(1);
        for cut in (0..full.len()).step_by(step) {
            let _ = read(&full[..cut]);
        }
        for i in (0..full.len()).step_by(step) {
            for flip in [0x01u8, 0x20, 0x80] {
                let mut v = full.clone();
                v[i] ^= flip;
                let _ = read(&v);
            }
        }
    }

    #[test]
    fn huge_counts_are_refused_before_anything_is_sized_from_them() {
        let started = std::time::Instant::now();
        let base = tri_doc("");
        for (from, to) in [
            (r#"count="9">"#, r#"count="4000000000">"#),
            (r#"count="3" stride="3""#, r#"count="4000000000" stride="3""#),
            (r#"<triangles count="1">"#, r#"<triangles count="4000000000">"#),
            (r#"stride="3""#, r#"stride="4000000000""#),
            (r#"offset="0"/><p>"#, r#"offset="4000000000"/><p>"#),
            ("<p>0 1 2</p>", "<p>0 1 4000000000</p>"),
            ("<p>0 1 2</p>", "<p>0 1 -2</p>"),
            ("0 0 0 1 0 0 0 1 0", "0 0 0 1 0 0 0 1 nan"),
            ("0 0 0 1 0 0 0 1 0", "0 0 0 1 0 0 0 1 1e999"),
        ] {
            assert!(base.contains(from), "{from}");
            let r = read_doc(&base.replacen(from, to, 1));
            assert!(matches!(r, Err(MeshError::Malformed(_)) | Err(MeshError::Unsupported(_))), "{to}: {r:?}");
        }
        assert!(started.elapsed() < std::time::Duration::from_secs(2), "{:?}", started.elapsed());
    }

    #[test]
    fn error_messages_quote_a_bounded_amount_of_input() {
        let id = "x".repeat(1 << 20);
        let body = tri_doc("").replace(r##"url="#g""##, &format!(r##"url="#{id}""##));
        let e = read_doc(&body).unwrap_err().to_string();
        assert!(e.len() < 1000, "{} bytes", e.len());
    }

    #[test]
    fn a_document_with_no_triangles_is_empty() {
        let body = scene_doc(&geom("g", &TRI, ""), r##"<node id="n"><instance_geometry url="#g"/></node>"##);
        assert_eq!(read_doc(&body).unwrap_err(), MeshError::Empty);
        assert!(matches!(read_doc(""), Err(MeshError::Empty) | Err(MeshError::Malformed(_))));
    }

    #[test]
    fn deeply_nested_animations_index_their_channels_in_linear_time() {
        // 500 nested <animation>s around 100k channels: indexing every channel
        // under every enclosing animation used to cost 50M entries here.
        let mut body = String::from("<library_animations>");
        for i in 0..500 {
            body.push_str(&format!(r#"<animation id="a{i}">"#));
        }
        for _ in 0..100_000 {
            body.push_str(r#"<channel source="" target=""/>"#);
        }
        for _ in 0..500 {
            body.push_str("</animation>");
        }
        body.push_str("</library_animations>");
        body.push_str(&tri_doc(""));
        let started = std::time::Instant::now();
        let _ = read_doc(&body);
        assert!(started.elapsed() < std::time::Duration::from_secs(5), "took {:?}", started.elapsed());
    }

    #[test]
    fn a_node_with_an_absurd_transform_stack_is_refused() {
        let stacked = |n: usize| tri_doc(&"<translate>0 0 0</translate>".repeat(n));
        assert!(read_doc(&stacked(MAX_NODE_OPS)).is_ok(), "the ceiling itself is allowed");
        let e = read_doc(&stacked(MAX_NODE_OPS + 1)).unwrap_err();
        assert!(e.to_string().contains("transform elements"), "{e}");
    }
}
