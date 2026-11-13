//! Format-neutral 3D mesh conversion.
//!
//! # Why this exists
//!
//! 3D vendors disagree about output containers. Meshy returns glb, obj, fbx,
//! stl and usdz from one job; Rodin returns exactly one of five; Stability and
//! most aggregator-hosted models (fal, Replicate, Runware, Segmind) return GLB
//! and nothing else. Without conversion, `output_formats` on an aggregator
//! model could only ever be `[glb]` — which is where litegen is most likely to
//! ship its first real 3D adapter, and so exactly where the param would be
//! useless.
//!
//! # What it converts
//!
//! Eleven containers, every pair, both directions: **glb, gltf, obj, stl, ply,
//! 3mf, dae, fbx, usdz, usda, usdc**. These are what 3D vendors deliver and what
//! DCC tools, game engines, AR viewers and printers take. All of it is
//! in-process Rust, with no external tool, no SDK and no licence attached.
//! FBX is written in the Autodesk SDK's own binary layout, and USDZ in
//! Apple's package rules; see `fbx` and `usd`.
//!
//! Every container is read into one scene (`scene`), shaped like glTF and
//! using glTF's conventions:
//!
//! - a node hierarchy with TRS transforms, and meshes of primitives;
//! - metallic-roughness materials with embedded PNG or JPEG textures;
//! - skins, morph targets and keyframed animation;
//! - glTF's punctual lights (directional, point, spot) and cameras
//!   (perspective, orthographic), placed by nodes;
//! - Y up, metres, V down and linear colour.
//!
//! Each codec maps its container's conventions to the scene's on read and
//! back on write. A conversion is read, then write; no pair of formats has
//! its own converter.
//!
//! # What a conversion keeps, and what it reports
//!
//! What a target cannot hold is a property of the container, and callers are
//! told rather than surprised:
//!
//! - `MeshFormat::capabilities` says what a container carries.
//! - `MeshFormat::prepare` produces the scene exactly as the target will
//!   encode it.
//! - `write_scene` returns the bytes together with one `Loss` per thing
//!   dropped, each with a stable, machine-readable `LossKind`.
//!
//! | target | keeps | material layers kept (the rest reported) | lights | cameras | drops, each reported | points | lines |
//! |--------|-------|------------------------------------------|--------|---------|----------------------|--------|-------|
//! | glb, gltf | everything the scene holds | all nine: clearcoat, transmission, volume, ior, specular, sheen, iridescence, anisotropy, dispersion | all, as `KHR_lights_punctual` (in `extensionsUsed` only) | all, as core `cameras` | — | yes, mode 0 | yes, mode 1 (a loop or strip read is expanded to segments) |
//! | fbx | hierarchy, normals, 8 uv sets (those its textures sample first), vertex colour, Phong materials (Blender's PBR mapping) with embedded textures, skins, morph targets, every animation clip (one stack each) | none written | all, as `Light` attributes turned to FBX's −Y, colours in `FbxColorSpace`, strengths in `LightUnits` (`photometric`, the default: `Intensity` is 100 × the candela or lux; `blender`: 100 × the `energy` Blender's glTF import gives the light), a range as far attenuation | all, as `Camera` attributes turned to FBX's +X; an unset aspect ratio, an infinite far plane and a mirrored orthographic view become 16:9, 1000 m and unmirrored, each reported | texture filters, texture transforms (baked into the uvs), metallic-roughness maps (their mean folded into the factors), and what `fbx::losses` names: double-sided, alpha mask, unlit, mirrored wrap, a tint beside a texture | none (reported); no portable FBX point type | none (reported); `Line` geometry is read, not written |
//! | usdz, usda, usdc | hierarchy, normals, uv sets (usdz 2, those its textures sample first; usda and usdc 8), vertex colour, PBR materials, unlit ones included (UsdPreviewSurface has none, so each is written as a recipe and a litegen marker, and reads back unlit), and textures (usda and usdc: sidecars, package only), skins, morph targets, animation (a usda or usdc: every clip, as the variant set `Animation`; a usdz: the first clip unless `usd_clips` is `all`) | clearcoat and ior; an unmapped clearcoat of 0 and an ior of 1.5 read back unset, so they go unreported | all, as `DistantLight` and `SphereLight` (with `ShapingAPI` for a spot), strengths in `LightUnits` (`photometric`, the default: the schema's units; `blender`: what Blender's USD exporter writes for the `energy` its glTF import gives the light); the exact values in litegen's `customData`, a range among them, which UsdLux has no form for, so it is reported for other readers | all, as `Camera`; an unset aspect ratio, an infinite far plane and a mirrored orthographic view become 16:9, 1000 m and unmirrored for other readers, each reported (`customData` keeps them) | the rigid motion of every clip but the first (under `usd_clips=all` each clip's skeletal and blend-shape motion is kept; under `first`, and for a scene whose clips are all rigid, every clip but the first is dropped), tangents, texture filters (transforms baked into the uvs); not unlit, which is kept as a recipe and a marker | yes, `Points` with `widths` | yes, linear `BasisCurves` (a cubic one is left out of a read with a warning), with `widths` |
//! | dae | hierarchy, normals, 8 uv sets (those its textures sample first), vertex colour, Phong materials and textures (sidecars, package only), skins, every animation clip | ior, less one of 0, 1 or 1.45, each reported (1.5 goes unreported) | all, with `<color>` the colour × intensity; the exact values in litegen's `<extra>`, a range among them, which Collada has no form for, so it is reported for other readers | all; an infinite far plane becomes 1000 m for other readers, reported (the `<extra>` keeps it) | morph targets (applied at their weights unless written with `dae_morphs: keep`: three.js and assimp fail on any `<morph>`), texture filters and transforms | none (reported); Collada 1.4.1 has no point primitive | yes, `<lines>` (`<linestrips>` is read, never written) |
//! | 3mf | placements (instancing), 1 uv set (the one its texture uses), vertex colour, base colour and base-colour texture | none | none (reported) | none (reported) | normals, skins, morph targets and animation (posed at rest), other material properties | none (reported) | none (reported) |
//! | obj | named parts, normals, 1 uv set (the one its textures use), vertex colour, Phong materials and textures through an MTL (package only) | package: clearcoat, sheen, ior and anisotropy, as factors; one file: none | none (reported) | none (reported) | hierarchy (flattened), skins, morphs and animation (posed at rest); as one file, materials become vertex colours and textures go | yes, `p` in an object of its own, with no normal or texture coordinate (a package draws them with an untextured copy of a textured material; both reported) | yes, one `l a b` per segment in an object of its own, with its texture coordinates but no normal (the normals reported) |
//! | ply | one mesh, normals, 1 uv set (the first), vertex colour | none | none (reported) | none (reported) | textures (a flat colour becomes vertex colour), hierarchy, animation | only in a file with no faces or edges (beside them, dropped and reported) | yes, an `edge` element |
//! | stl | one mesh of triangles | none | none (reported) | none (reported) | everything else (facet normals are derived) | none (reported) | none (reported) |
//!
//! # One file, or a package
//!
//! OBJ's MTL and images, glTF's buffers and images, and DAE's, USDA's and
//! USDC's images live beside the model. `WriteOptions::self_contained` chooses:
//!
//! - **One file** (the default, and what `convert` returns) embeds where
//!   the container can: glb, gltf as data URIs, fbx, usdz and 3mf. Where it
//!   cannot, the loss is reported.
//! - **A package** writes the sidecars too, as files of the returned
//!   `Package`. `zip::write_package` turns it into one zip, for a caller that
//!   ships it as one download.
//!
//! litegen's converter and its 3D delivery ship one file by default, and the
//! package as a zip on request. Readers resolve sidecars through
//! `package::Sidecars`.
//!
//! # How it is verified
//!
//! - **Unit tests per codec**, including files written by independent tools
//!   (in `testdata/`): assimp, SceneKit, Apple's USD tools, Pixar's
//!   `usdcat` and three.js's USDZ exporter.
//! - **`scene_matrix_tests`** runs every fixture through every pair, in both
//!   write modes. It compares area, signed volume, bounds and centroid at
//!   rest and at every animation sample time.
//! - **`scripts/mesh-verify`** renders every conversion with an independent
//!   loader for its format (three.js, with SceneKit and Pixar's usdrecord as
//!   second renderers), compares the pictures with the expected scene's
//!   pixel by pixel, and validates every file with the Khronos glTF
//!   validator, assimp and usdchecker. Third-party models run through it
//!   too, but are not committed: the Khronos glTF samples, and FBX files
//!   from Mixamo, Maya, 3ds Max and Blender.
//!
//! The method and results are in `docs/superpowers/3d-format-conversion.md`.
//!
//! # Untrusted input
//!
//! Every reader takes hostile bytes:
//!
//! - counts are checked against the data actually present before anything is
//!   sized from them;
//! - decoding and resampling work is budgeted;
//! - nesting is walked with explicit stacks;
//! - a malformed file is an error, never a panic or a hang.
//!
//! # Isolation
//!
//! This module imports NOTHING from the rest of litegen — no types, no
//! providers, no storage. It is bytes in, bytes out, and `mesh_isolation_tests`
//! enforces that mechanically so it stays directly unit-testable (and
//! extractable into its own crate) as the codebase moves around it.

pub mod humanoid;
pub mod inspect;
pub mod ir;
pub mod ktx2;
pub mod normalize;
pub mod math;
pub mod polygon;
pub mod pose;
pub mod scene;
#[doc(hidden)]
pub mod fixtures;

pub mod glb;
pub mod package;
pub mod xml;
pub mod zip;
pub mod dae;
pub mod fbx;
mod fbx_dom;
mod fbx_read;
mod fbx_write;
pub mod obj;
pub mod ply;
pub mod stl;
pub mod threemf;
pub mod usd;
mod usd_read;
mod usd_stage;
mod usd_write;
mod usda;
mod usdc;
mod meshopt;
mod draco;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod matrix_tests;
#[cfg(test)]
mod scene_matrix_tests;
#[cfg(test)]
mod isolation_tests;

pub use ir::Mesh;
pub use package::{NoSidecars, Package, PackageFile, Sidecars};
pub use scene::{Capabilities, Loss, LossKind, MaterialSupport, Scene};

use std::fmt;

/// A container this module can read and write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MeshFormat {
    Glb,
    /// glTF as JSON. Written self-contained (one file, buffer embedded as a
    /// data URI) through the single-file API, or as `.gltf` + `.bin` + images
    /// through [`write_scene`] with `self_contained: false`.
    Gltf,
    Obj,
    Stl,
    Ply,
    /// 3D Manufacturing Format: an OPC (ZIP) package, always one file with its
    /// textures inside. Z-up and unit-tagged; see `threemf` for the mapping.
    ThreeMf,
    /// COLLADA 1.4.1. Textures travel as `textures/*` sidecars, so a
    /// self-contained document carries none (see `dae`).
    Dae,
    /// Autodesk FBX: binary 7100–7700 and ASCII in, binary 7400 out, textures
    /// embedded (see `fbx`).
    Fbx,
    /// USD packaged as a ZIP: one file, textures inside. Root layer written as
    /// USDA, or as a crate (`model.usdc`) when [`WriteOptions::usdz_root`] is
    /// [`UsdzRoot::Usdc`]; USDA and USDC roots read.
    Usdz,
    /// A standalone USD layer: USDA text written, USDA or USDC crate read.
    /// Textures are sidecar files through [`write_scene`] with
    /// `self_contained: false`, absent otherwise.
    Usda,
    /// A standalone USD layer as a binary crate (USDC, version 0.8.0): the
    /// same layer [`MeshFormat::Usda`] writes, encoded as a crate, so it
    /// holds what a USDA does. Textures are sidecars, as for USDA. USDA or
    /// USDC reads: a crate is one layer like a text one.
    Usdc,
}

impl MeshFormat {
    /// Every format, in a stable order. The conversion matrix iterates this, so
    /// adding a variant automatically adds a row and a column of coverage.
    pub const ALL: [MeshFormat; 11] = [
        MeshFormat::Glb,
        MeshFormat::Gltf,
        MeshFormat::Obj,
        MeshFormat::Stl,
        MeshFormat::Ply,
        MeshFormat::ThreeMf,
        MeshFormat::Dae,
        MeshFormat::Fbx,
        MeshFormat::Usdz,
        MeshFormat::Usda,
        MeshFormat::Usdc,
    ];

    /// The lowercase extension, without a dot — the same spelling litegen uses
    /// in `Model3dAsset.format` and in storage keys.
    pub fn as_str(self) -> &'static str {
        match self {
            MeshFormat::Glb => "glb",
            MeshFormat::Gltf => "gltf",
            MeshFormat::Obj => "obj",
            MeshFormat::Stl => "stl",
            MeshFormat::Ply => "ply",
            MeshFormat::Usdz => "usdz",
            MeshFormat::Usda => "usda",
            MeshFormat::Usdc => "usdc",
            MeshFormat::ThreeMf => "3mf",
            MeshFormat::Dae => "dae",
            MeshFormat::Fbx => "fbx",
        }
    }

    /// Parse an extension. Case-insensitive; `None` for anything this module
    /// cannot handle, which callers must treat as "not convertible" rather than
    /// falling back to some default.
    pub fn parse(s: &str) -> Option<MeshFormat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "glb" => Some(MeshFormat::Glb),
            "gltf" => Some(MeshFormat::Gltf),
            "obj" => Some(MeshFormat::Obj),
            "stl" => Some(MeshFormat::Stl),
            "ply" => Some(MeshFormat::Ply),
            "usdz" => Some(MeshFormat::Usdz),
            // `.usd` stays unparsed: it names either encoding, and a format
            // name must mean one, in both directions. `usda` and `usdc` each
            // write their own and read either.
            "usda" => Some(MeshFormat::Usda),
            "usdc" => Some(MeshFormat::Usdc),
            "3mf" => Some(MeshFormat::ThreeMf),
            "dae" => Some(MeshFormat::Dae),
            "fbx" => Some(MeshFormat::Fbx),
            _ => None,
        }
    }

    /// Whether this container, written as a package (`self_contained:
    /// false`), can have files beside its primary: an obj its `.mtl` and
    /// textures, a dae, usda or usdc its textures, a gltf its `.bin` and
    /// images. Every other container is one file in either form, so a caller
    /// that ships packages need not write it twice.
    pub fn writes_sidecars(self) -> bool {
        matches!(self, MeshFormat::Obj | MeshFormat::Dae | MeshFormat::Usda | MeshFormat::Usdc | MeshFormat::Gltf)
    }

    /// What this container can hold — one row of the lossiness table.
    ///
    /// `opts` matters for the containers whose richest form is several files:
    /// a self-contained OBJ has no `.mtl` to put materials in, for instance.
    pub fn capabilities(self, opts: WriteOptions) -> Capabilities {
        match self {
            // Everything the scene holds, points and lines (modes 0 and 1) too.
            MeshFormat::Glb | MeshFormat::Gltf => Capabilities::full(),
            // World-space objects with per-material primitives. Materials need
            // the `.mtl` sidecar; one self-contained file bakes each material's
            // colour into its vertices instead.
            MeshFormat::Obj if opts.self_contained => Capabilities {
                normals: true,
                uv_sets: 1,
                vertex_colors: true,
                materials: MaterialSupport::VertexColor,
                multiple_meshes: true,
                // `l` and `p` statements, each kind in an object of its own.
                points: true,
                lines: true,
                ..Capabilities::none()
            },
            MeshFormat::Obj => Capabilities {
                normals: true,
                uv_sets: 1,
                vertex_colors: true,
                materials: MaterialSupport::Phong,
                textures: true,
                multiple_meshes: true,
                points: true,
                lines: true,
                // Blender's PBR extension to MTL; see `obj`.
                layers: scene::LayerSupport { clearcoat: true, sheen: true, ior: true, anisotropy: true, ..scene::LayerSupport::none() },
                ..Capabilities::none()
            },
            // One merged mesh. Points are held, but only in a file with no
            // faces or edges, which no flag describes: see `ply::prepare`.
            MeshFormat::Ply => Capabilities {
                normals: true,
                uv_sets: 1,
                vertex_colors: true,
                materials: MaterialSupport::VertexColor,
                points: true,
                lines: true,
                ..Capabilities::none()
            },
            MeshFormat::Stl => Capabilities::none(),
            MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc => usd::capabilities(self, opts),
            MeshFormat::Dae => dae::capabilities(opts),
            MeshFormat::Fbx => fbx::capabilities(),
            // Placed parts with per-triangle colour: base materials, colour
            // groups and one texture (V-flipped coordinates, wrap modes, an
            // auto/linear/nearest filter). No normals, nothing that moves.
            MeshFormat::ThreeMf => Capabilities {
                hierarchy: true,
                uv_sets: 1,
                vertex_colors: true,
                materials: MaterialSupport::ColorOnly,
                textures: true,
                // Its filter attribute (auto/linear/nearest); what does not
                // map is `threemf::losses`'.
                texture_filters: true,
                texture_wrap: true,
                multiple_meshes: true,
                ..Capabilities::none()
            },
        }
    }

    /// The scene reduced to exactly what this container's writer encodes.
    ///
    /// Writers are handed this, and the render verification harness renders
    /// `to.prepare(from.prepare(scene))` as the expected picture of a
    /// conversion — so the lossiness table and the writers cannot disagree
    /// without a test noticing.
    pub fn prepare(self, scene: &Scene, opts: WriteOptions) -> Result<Scene, MeshError> {
        // 3MF keeps placements and instancing, which `pose::degrade` would bake
        // away with the skins, and gives each triangle one property group,
        // which no capability flag describes. It holds no lights or cameras.
        if self == MeshFormat::ThreeMf {
            let mut s = threemf::prepare(scene)?;
            pose::drop_lights_and_cameras(&mut s, &self.capabilities(opts));
            return Ok(s);
        }
        // Collada keeps skins and hierarchy while baking morph targets, which
        // the generic reduction cannot express (see `dae::prepare`).
        if self == MeshFormat::Dae {
            return dae::prepare(scene, opts);
        }
        // FBX's material and sampler model needs more than the generic
        // reduction; see `fbx::prepare`.
        if self == MeshFormat::Fbx {
            return fbx::prepare(scene, opts);
        }
        // An MTL holds a layer's factors but not its maps; see `obj::prepare`.
        if self == MeshFormat::Obj {
            return obj::prepare(scene, opts);
        }
        // PLY keeps points only in a file with no faces or edges, which no
        // capability flag describes; see `ply::prepare`.
        if self == MeshFormat::Ply {
            return ply::prepare(scene, opts);
        }
        let caps = self.capabilities(opts);
        if matches!(self, MeshFormat::Glb | MeshFormat::Gltf) {
            // glTF says only TRS (§5.25): a sheared matrix is baked or
            // approximated as `pose::bake_shear` describes.
            // Split skinned meshes as the writer does first, so a rigid part
            // under a shear is baked, not left under an approximated parent.
            //
            // glTF's row keeps everything the scene holds, points and lines
            // included, so `degrade` (which also prunes the textures no
            // material uses, and the tangents of a primitive without uvs)
            // runs only if the row lacks something.
            let degraded = if caps == Capabilities::full() { None } else { Some(pose::degrade(scene, &caps)?) };
            let mut s = glb::hoist_skinned_meshes(degraded.as_ref().unwrap_or(scene));
            pose::bake_shear(&mut s);
            // A directional light keeps no range: glTF applies one to point
            // and spot lights only, and the writer leaves it out
            // (`glb::light_json`), as FBX's, Collada's and USD's `prepare`
            // drop it (Ruling R13).
            for l in s.lights.iter_mut().filter(|l| l.kind == scene::LightKind::Directional) {
                l.range = None;
            }
            return Ok(s);
        }
        if matches!(self, MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc) {
            // USD's writer encodes less than the generic degrade describes:
            // one timeline, linear keys, PNG/JPEG, joints-only skeletons.
            return usd::prepare(scene, &caps, opts.key_rate(), opts.clips_for(self));
        }
        if caps == Capabilities::full() {
            return Ok(scene.clone());
        }
        pose::degrade(scene, &caps)
    }

    /// Whether this container carries the flat mesh's texture coordinates.
    ///
    /// 3MF stores them only inside a texture's coordinate group, and the flat
    /// mesh has no texture to put them in.
    pub fn keeps_uvs(self) -> bool {
        self != MeshFormat::ThreeMf && self.capabilities(WriteOptions::default()).uv_sets > 0
    }

    /// Whether this container carries any colour at all without a sidecar.
    pub fn keeps_color(self) -> bool {
        let c = self.capabilities(WriteOptions::default());
        c.materials != MaterialSupport::None || c.vertex_colors
    }
}

impl fmt::Display for MeshFormat {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MeshError {
    /// The bytes are not this container at all (bad magic, bad header).
    NotThisFormat { expected: MeshFormat, detail: String },
    /// The container parsed but its contents are invalid or unsupported.
    Malformed(String),
    /// Valid, but uses a feature this module deliberately does not model.
    Unsupported(String),
    /// The input draws nothing: no triangles, no lines and no points (the flat
    /// API, which holds triangles alone, counts triangles). Always an error:
    /// an empty mesh is never a useful deliverable, and passing it on turns a
    /// vendor failure into a file the caller has to diagnose themselves.
    Empty,
}

impl fmt::Display for MeshError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            MeshError::NotThisFormat { expected, detail } => {
                write!(f, "not a {expected} file: {detail}")
            }
            MeshError::Malformed(d) => write!(f, "malformed mesh: {d}"),
            MeshError::Unsupported(d) => write!(f, "unsupported mesh feature: {d}"),
            MeshError::Empty => f.write_str("mesh contains no triangles, lines or points"),
        }
    }
}

impl std::error::Error for MeshError {}

/// Largest input this module will parse, in bytes.
///
/// Conversion is CPU and memory work on the poll path, and its memory grows
/// with the input. Measured peak heap, counting the input itself
/// (`examples/mesh_peak.rs`, on seven vendor meshes of 1.3–26 MB, 2026-11-01):
///
/// - reading: 2–9 times the input (a USDZ decodes its textures);
/// - one conversion, with the input, the scene and one output alive: 5–13
///   times;
/// - writing all ten containers measured then (usdc came after) from one read
///   and holding them: 12–31 times.
///
/// At this cap that is about 1.7 GB for one conversion and 4 GB for all ten,
/// so a caller bounds how many it runs at once. A compressed container's
/// reader also stops at `MAX_EXPANDED_BYTES`, 1 GiB. Callers hitting this cap
/// should decline the conversion, not retry.
pub const MAX_INPUT_BYTES: usize = 128 * 1024 * 1024;

/// The most a compressed container may expand to while it is read: FBX's
/// zlib arrays, decoded, and a zip package's entries (3MF, USDZ), inflated.
/// Eight times the input cap, 1 GiB.
///
/// It is sized so that the scene caps, not this budget, bound what litegen
/// reads back of its own files. What litegen's FBX and 3MF expand to grows
/// with vertices and corners; for a dense textured mesh at the caps (4M
/// vertices, 12M corners, every normal, tangent, UV and colour distinct) it is
/// about 0.85 GB (`our_own_fbx_and_3mf_of_any_scene_within_the_caps_fit_their_
/// expansion_budgets`). It is also a fixed ceiling on a decompression bomb, of
/// the same order as the memory a scene at the caps takes anyway.
const MAX_EXPANDED_BYTES: usize = 8 * MAX_INPUT_BYTES;

fn check_size(bytes: &[u8]) -> Result<(), MeshError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(MeshError::Unsupported(format!(
            "input is {} bytes, over the {MAX_INPUT_BYTES}-byte limit",
            bytes.len()
        )));
    }
    Ok(())
}

// ─── scene API ──────────────────────────────────────────────────────────────

/// A scene read from some container, and what the reader recognised but did
/// not carry (points and lines, textures whose files were not supplied, ...).
#[derive(Debug, Clone)]
pub struct SceneRead {
    pub scene: Scene,
    pub warnings: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WriteOptions {
    /// Produce exactly one file. Containers that normally reference sidecars
    /// either embed them (glTF) or leave them out and report the loss (OBJ's
    /// `.mtl`, Collada's images).
    pub self_contained: bool,
    /// The rate, in frames per second, that a container which resamples
    /// animation (FBX, Collada, USD) keys it at and, where it can say one,
    /// declares (FBX `TimeMode`, USD `framesPerSecond`): the delivery's `fps`.
    /// `None` is 30, Mixamo's and the FBX SDK's default `TimeMode`.
    ///
    /// Resampling pins the source's own interpolation down at a rate where a
    /// container's cannot drift from it visibly: FBX rotation curves
    /// interpolate Euler angles componentwise, while three.js slerps between
    /// whatever keys it gets. Near gimbal lock no fixed rate is enough, and
    /// the FBX writer refines those spans (`fbx_write::refine_euler_times`).
    pub fps: Option<u32>,
    /// How FBX material and light colours are written ([`FbxColorSpace`]).
    pub fbx_color_space: FbxColorSpace,
    /// The units an FBX file's lights are written in ([`LightUnits`]).
    pub fbx_light_units: LightUnits,
    /// The units a USD file's lights are written in ([`LightUnits`]).
    pub usd_light_units: LightUnits,
    /// How a Collada file's morph targets are written ([`DaeMorphs`]).
    pub dae_morphs: DaeMorphs,
    /// What a USDZ package's root layer is written as ([`UsdzRoot`]). Read
    /// only when writing [`MeshFormat::Usdz`].
    pub usdz_root: UsdzRoot,
    /// Which animation clips a USD file holds ([`UsdClips`]), for
    /// [`MeshFormat::Usda`], [`MeshFormat::Usdc`] and [`MeshFormat::Usdz`].
    /// `None` is the container's default: `all` for a USDA or USDC, `first`
    /// for a USDZ until an AR Quick Look device check passes.
    ///
    /// [`WriteOptions::clips_for`] resolves it for a container. `first` is
    /// the output a USD file always had; `all` is [`UsdClips::All`]'s layout.
    pub usd_clips: Option<UsdClips>,
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions {
            self_contained: true,
            fps: None,
            fbx_color_space: FbxColorSpace::Srgb,
            fbx_light_units: LightUnits::Photometric,
            usd_light_units: LightUnits::Photometric,
            dae_morphs: DaeMorphs::Bake,
            usdz_root: UsdzRoot::Usda,
            usd_clips: None,
        }
    }
}

/// How an FBX file's classic material colours (`DiffuseColor`,
/// `EmissiveColor`, a 3ds Max material's colours) and its lights' `Color`
/// are stored. FBX does not say, and its readers disagree
/// (`fbx::srgb_encode`), so the caller chooses. Vertex colours are sRGB
/// either way, Blender's default.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum FbxColorSpace {
    /// sRGB-encoded, as three.js's FBXLoader reads them.
    #[default]
    Srgb,
    /// The scene's linear values, as Blender, Unity URP, ufbx and assimp read
    /// them.
    Linear,
}

/// The units a file's light strengths are stored in. glTF's are photometric by
/// its specification, and so is the scene's [`scene::Light::intensity`]
/// (candela for a point or spot light, lux for a directional one). FBX and USD
/// say nothing, and Blender, the one reader whose answer matters most, does
/// not take them that way, so the caller chooses, for each of the two formats.
///
/// `Blender` writes each light with the `energy` Blender's own glTF importer
/// gives the same scene's glTF light (`scene::blender_energy`), so that it
/// opens in Blender as bright as the glTF does, and reads a light Blender
/// wrote as the glTF light whose import gives that `energy`
/// (`scene::from_blender_energy`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum LightUnits {
    /// The scene's own units, as a file always held them: FBX `Intensity` is
    /// 100 × the candela or lux (what ufbx and three.js's `FBXLoader` divide
    /// back out), and USD's are the normalised values `usd_write` documents.
    #[default]
    Photometric,
    /// Blender's radiometric `energy` (watts, or W/m² for a sun).
    Blender,
}

impl LightUnits {
    /// The option's name, as a caller spells it: `"photometric"` or
    /// `"blender"`.
    pub fn as_str(self) -> &'static str {
        match self {
            LightUnits::Photometric => "photometric",
            LightUnits::Blender => "blender",
        }
    }
}

/// How the Collada writer treats a scene's morph targets. Collada can hold them
/// as `<morph>` controllers, but only some readers open such a file (SceneKit
/// and Godot do; three.js and assimp refuse it), so the caller chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum DaeMorphs {
    /// Each node's mesh is written at that node's effective weights and the
    /// weight animation is dropped and reported: the output every Collada
    /// reader opens, and the one a file always had.
    #[default]
    Bake,
    /// The targets are written as morph controllers, with their weight
    /// animation.
    Keep,
}

impl DaeMorphs {
    /// The option's name, as a caller spells it: `"bake"` or `"keep"`.
    pub fn as_str(self) -> &'static str {
        match self {
            DaeMorphs::Bake => "bake",
            DaeMorphs::Keep => "keep",
        }
    }
}

/// What a USDZ package's root layer, its first entry, is written as. Both
/// pass `usdchecker --arkit` (the rule demanding a crate was removed in
/// OpenUSD 24.03), and every USD reader opens either, so the caller chooses.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum UsdzRoot {
    /// `model.usda`, text: what a USDZ here always was, and what
    /// `<model-viewer>` hands to Quick Look.
    #[default]
    Usda,
    /// `model.usdc`, the crate: how Apple's own packagers write a USDZ. Still
    /// the first entry, and its data still 64-byte aligned.
    Usdc,
}

impl UsdzRoot {
    /// The option's name, as a caller spells it: `"usda"` or `"usdc"`.
    pub fn as_str(self) -> &'static str {
        match self {
            UsdzRoot::Usda => "usda",
            UsdzRoot::Usdc => "usdc",
        }
    }
}

/// Which animation clips a USD file holds. USD has one timeline, so a scene
/// with several clips either keeps them as alternatives or keeps the first.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UsdClips {
    /// Every clip, as Apple's `variant_combiner.py` lays them out: a
    /// SkelAnimation prim for each, and a variant set `Animation` that picks
    /// which one a skeleton binds. Only skeletal and blend-shape motion
    /// switches: a clip's rigid motion beyond the first is reported lost, and
    /// a scene with nothing else than rigid motion keeps its first clip.
    All,
    /// The first clip; the rest are reported as lost.
    First,
}

impl UsdClips {
    /// The option's name, as a caller spells it: `"all"` or `"first"`.
    pub fn as_str(self) -> &'static str {
        match self {
            UsdClips::All => "all",
            UsdClips::First => "first",
        }
    }
}

/// How a reader takes what a container leaves unsaid.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadOptions {
    /// The colour space an FBX file's material and light colours are stored
    /// in.
    pub fbx_color_space: FbxColorSpace,
    /// The units an FBX file's light strengths are stored in.
    pub fbx_light_units: LightUnits,
    /// The units a USD file's light strengths are stored in.
    pub usd_light_units: LightUnits,
}

impl WriteOptions {
    /// [`WriteOptions::fps`] as the rate resampling uses.
    pub fn key_rate(&self) -> f32 {
        self.fps.filter(|f| *f > 0).map_or(30.0, |f| f as f32)
    }

    /// [`WriteOptions::usd_clips`] as `format` writes it: the word asked for,
    /// else the container's default, [`UsdClips::All`] for a USDA or USDC and
    /// [`USDZ_DEFAULT_CLIPS`] for a USDZ.
    pub fn clips_for(&self, format: MeshFormat) -> UsdClips {
        self.usd_clips.unwrap_or(if format == MeshFormat::Usdz { USDZ_DEFAULT_CLIPS } else { UsdClips::All })
    }
}

/// The clips a USDZ keeps when the caller does not say (`usd_clips` unset).
/// It is `First`, today's output, because how AR Quick Look plays a
/// variant-switched animation on a device is not established (research Q1 of
/// mesh-gaps item 6, whose device check was skipped). If a multi-clip USDZ
/// passes such a check, this flips to [`UsdClips::All`], and this is the one
/// place to change.
pub const USDZ_DEFAULT_CLIPS: UsdClips = UsdClips::First;

/// A written model and everything the target container could not hold.
#[derive(Debug, Clone)]
pub struct SceneWrite {
    pub package: Package,
    pub losses: Vec<Loss>,
}

/// A scene that draws nothing is [`MeshError::Empty`]. Triangles are not all
/// that is drawn: a file of points, or of lines, is a scene.
fn require_drawable(scene: &Scene) -> Result<(), MeshError> {
    if scene.drawable_count() == 0 {
        return Err(MeshError::Empty);
    }
    Ok(())
}

/// Whether `scene` has a primitive that a container with `caps` cannot hold,
/// which [`pose::strip_topologies`] would remove.
fn has_unheld_topology(scene: &Scene, caps: &Capabilities) -> bool {
    scene.meshes.iter().flat_map(|m| &m.primitives).any(|p| !caps.holds(p.topology))
}

/// The refusal for a scene that draws, but nothing its target can hold: points
/// to STL, say. It names the target, what it holds, and what the scene has
/// that it does not, so that the caller is told why instead of being given a
/// bare [`MeshError::Empty`]:
/// `stl holds triangles only; this scene has 9 points and no triangles`.
fn nothing_the_target_holds(scene: &Scene, to: MeshFormat, caps: &Capabilities) -> MeshError {
    // "a", "a and b", "a, b and c" (with `and` or `or`).
    fn list(items: &[String], joiner: &str) -> String {
        match items {
            [] => String::new(),
            [only] => only.clone(),
            [rest @ .., last] => format!("{} {joiner} {last}", rest.join(", ")),
        }
    }
    let count = |topology: scene::Topology| -> usize {
        scene
            .meshes
            .iter()
            .flat_map(|m| &m.primitives)
            .filter(|p| p.topology == topology)
            .map(|p| p.indices.len() / topology.indices_per_element())
            .sum()
    };
    let noun = |n: usize, one: &str, many: &str| format!("{n} {}", if n == 1 { one } else { many });
    let mut held = vec!["triangles".to_string()];
    let mut has = Vec::new();
    if caps.lines {
        held.push("lines".to_string());
    } else if count(scene::Topology::Lines) > 0 {
        has.push(noun(count(scene::Topology::Lines), "line segment", "line segments"));
    }
    if caps.points {
        held.push("points".to_string());
    } else if count(scene::Topology::Points) > 0 {
        has.push(noun(count(scene::Topology::Points), "point", "points"));
    }
    let holds = if held.len() == 1 { "triangles only".to_string() } else { list(&held, "and") };
    MeshError::Unsupported(format!("{to} holds {holds}; this scene has {} and no {}", list(&has, "and"), list(&held, "or")))
}

/// Read any supported container into a scene.
pub fn read_scene(bytes: &[u8], from: MeshFormat, sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
    read_scene_with(bytes, from, sidecars, ReadOptions::default())
}

/// [`read_scene`], taking what the file leaves unsaid as `opts` says.
pub fn read_scene_with(bytes: &[u8], from: MeshFormat, sidecars: &dyn Sidecars, opts: ReadOptions) -> Result<SceneRead, MeshError> {
    check_size(bytes)?;
    let (scene, warnings) = match from {
        MeshFormat::Glb => {
            let o = glb::read_scene(bytes, sidecars)?;
            (o.scene, o.warnings)
        }
        MeshFormat::Gltf => {
            let o = glb::read_gltf_scene(bytes, sidecars)?;
            (o.scene, o.warnings)
        }
        MeshFormat::Obj => {
            let r = obj::read_scene(bytes, sidecars)?;
            (r.scene, r.warnings)
        }
        MeshFormat::Stl => stl::read_scene(bytes)?,
        MeshFormat::Ply => ply::read_scene_warned(bytes)?,
        MeshFormat::Usdz => {
            let r = usd::read_usdz_with(bytes, sidecars, opts.usd_light_units)?;
            (r.scene, r.warnings)
        }
        MeshFormat::Usda => {
            let r = usd::read_usda_with(bytes, sidecars, opts.usd_light_units)?;
            (r.scene, r.warnings)
        }
        MeshFormat::Usdc => {
            let r = usd::read_usdc_with(bytes, sidecars, opts.usd_light_units)?;
            (r.scene, r.warnings)
        }
        MeshFormat::ThreeMf => {
            let r = threemf::read_scene(bytes, sidecars)?;
            (r.scene, r.warnings)
        }
        MeshFormat::Dae => {
            let r = dae::read_scene(bytes, sidecars)?;
            (r.scene, r.warnings)
        }
        MeshFormat::Fbx => {
            let r = fbx::read_scene_with(bytes, sidecars, opts.fbx_color_space, opts.fbx_light_units)?;
            (r.scene, r.warnings)
        }
    };
    scene.validate()?;
    require_drawable(&scene)?;
    Ok(SceneRead { scene, warnings })
}

/// Write a scene as any supported container, reporting what it cannot hold.
pub fn write_scene(scene: &Scene, to: MeshFormat, opts: WriteOptions) -> Result<SceneWrite, MeshError> {
    scene.validate()?;
    require_drawable(scene)?;
    if matches!(to, MeshFormat::Fbx | MeshFormat::Dae) {
        // These key every animated node of every clip on a 30 fps grid;
        // refused here, before `prepare` or the writer samples anything. USD
        // keeps LINEAR keys as authored and budgets what it does resample
        // (cubic channels, re-parented joints) where it resamples it.
        // A weight channel costs a value a target and time, except where
        // Collada keeps its morphs: there each primitive and target gets an
        // `<animation>` of a TIME, a WEIGHT and an INTERPOLATION array.
        let keeps_morphs = matches!(to, MeshFormat::Dae) && opts.dae_morphs == DaeMorphs::Keep;
        pose::check_resample_budget(scene, opts.key_rate(), |m| {
            let per_target = if keeps_morphs { m.primitives.len().saturating_mul(3) } else { 1 };
            m.target_count().saturating_mul(per_target)
        })?;
    }
    // Geometry counted as each writer copies it, before it copies anything:
    // USD writes every instance's mesh inline; FBX gives a skinned or morphed
    // node its own geometry; 3MF poses skins and morphs, and bakes a sheared
    // placement, per instance; Collada bakes morphs per instance under
    // `DaeMorphs::Bake`, but under `Keep` copies nothing per instance: its
    // controllers, one for each set of node weights, share the base and
    // target geometries. The others reference one mesh from every node, or
    // merge through `pose::world_primitives`, which checks as it goes.
    // glTF and FBX copy each mesh they bake out of a sheared transform.
    let morphed = |n: usize| scene.nodes[n].mesh.is_some_and(|m| scene.meshes[m].target_count() > 0);
    let deformed = |n: usize| scene.nodes[n].skin.is_some() || morphed(n);
    let shear_baked = |to: MeshFormat| -> std::collections::HashSet<usize> {
        match to {
            MeshFormat::Glb | MeshFormat::Gltf | MeshFormat::Fbx => pose::shear_plan(scene).baked.iter().map(|b| b.0).collect(),
            _ => Default::default(),
        }
    };
    let baked = shear_baked(to);
    match to {
        MeshFormat::Glb | MeshFormat::Gltf if !baked.is_empty() => pose::check_copied_geometry(scene, |n| baked.contains(&n))?,
        MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc => pose::check_copied_geometry(scene, |_| true)?,
        MeshFormat::Fbx => pose::check_copied_geometry(scene, |n| deformed(n) || baked.contains(&n))?,
        MeshFormat::Dae => match opts.dae_morphs {
            DaeMorphs::Bake => pose::check_copied_geometry(scene, morphed)?,
            // No instance is copied, but the call is not empty: it counts
            // every mesh once against the scene-wide vertex and index
            // ceilings, which the base geometries Keep writes still answer to.
            DaeMorphs::Keep => pose::check_copied_geometry(scene, |_| false)?,
        },
        MeshFormat::ThreeMf => {
            let world = scene.world_matrices();
            let sheared = |n: usize| math::decompose(&world[n]).is_none() || math::det3(&math::upper3(&world[n])).abs() <= 1e-12;
            pose::check_copied_geometry(scene, |n| deformed(n) || sheared(n))?
        }
        _ => {}
    }
    let caps = to.capabilities(opts);
    let mut losses = scene::losses(scene, &caps, to.as_str());
    // What a writer adds to the capability table is judged on what it is
    // given: the scene less the points and lines `prepare` removes, which
    // `scene::losses` has named, and which the codecs' own accounts (a
    // loop's eight indices as 3MF's triangles, say) were not written for.
    let held;
    let judged = if has_unheld_topology(scene, &caps) {
        let mut copy = scene.clone();
        pose::strip_topologies(&mut copy, &caps);
        held = copy;
        &held
    } else {
        scene
    };
    if to == MeshFormat::ThreeMf {
        losses.extend(threemf::losses_after_strip(judged, scene));
    }
    if to == MeshFormat::Dae {
        losses.extend(dae::extra_losses(judged, opts));
    }
    if to == MeshFormat::Fbx {
        // What FBX's material and sampler model cannot say, beyond the
        // capability table, and the shear its TRS transforms cannot.
        losses.extend(fbx::losses(judged));
        losses.extend(pose::shear_losses(judged, to.as_str()));
    }
    if matches!(to, MeshFormat::Glb | MeshFormat::Gltf) {
        losses.extend(glb::losses(judged, to.as_str()));
    }
    if to == MeshFormat::Obj {
        losses.extend(obj::losses(judged, opts.self_contained));
    }
    if to == MeshFormat::Ply {
        losses.extend(ply::losses(judged));
    }
    if matches!(to, MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc) {
        losses.extend(usd::losses(judged, to.as_str(), caps.textures, opts.clips_for(to)));
    }
    // Something is drawn (`require_drawable`), but nothing the target can
    // hold: named, before `prepare` or a writer has anything to do, rather than
    // left to the writer's bare `Empty`.
    if judged.drawable_count() == 0 {
        return Err(nothing_the_target_holds(scene, to, &caps));
    }
    let prepared = to.prepare(scene, opts)?;
    let package = match to {
        MeshFormat::Glb => Package::single("model.glb", glb::write_scene(&prepared)?),
        MeshFormat::Gltf => glb::write_gltf_scene(&prepared, opts.self_contained)?,
        MeshFormat::Obj => obj::write_scene(&prepared, opts.self_contained)?,
        // STL's writer takes the flat triangle mesh, which has no topology: a
        // line or point cannot reach it (`pose::flatten` keeps triangles
        // alone), so there is no debug assertion to make here, as there is in
        // the 3MF, FBX and Collada writers (`pose::debug_assert_holds`).
        MeshFormat::Stl => Package::single("model.stl", stl::write(&pose::flatten(&prepared)?)?),
        MeshFormat::Ply => Package::single("model.ply", ply::write_scene(&prepared)?),
        MeshFormat::Usdz => usd::write_usdz(&prepared, opts)?,
        MeshFormat::Usda => usd::write_usda(&prepared, opts)?,
        MeshFormat::Usdc => usd::write_usdc(&prepared, opts)?,
        MeshFormat::ThreeMf => threemf::write_scene(&prepared)?,
        MeshFormat::Dae => dae::write_scene(&prepared, opts)?,
        // Textures are always embedded: FBX's own convention, and the only
        // form every FBX reader resolves without the original directory.
        MeshFormat::Fbx => Package::single("model.fbx", fbx::write_scene(&prepared, opts.key_rate(), opts.fbx_color_space, opts.fbx_light_units)?),
    };
    Ok(SceneWrite { package, losses })
}

/// Convert between containers at full fidelity: everything the target can
/// hold survives, and everything it cannot is named in `losses`.
pub fn convert_scene(
    bytes: &[u8],
    from: MeshFormat,
    sidecars: &dyn Sidecars,
    to: MeshFormat,
    opts: WriteOptions,
) -> Result<SceneWrite, MeshError> {
    // One choice of FBX colour space and of light units, at both ends.
    let read = read_scene_with(
        bytes,
        from,
        sidecars,
        ReadOptions {
            fbx_color_space: opts.fbx_color_space,
            fbx_light_units: opts.fbx_light_units,
            usd_light_units: opts.usd_light_units,
        },
    )?;
    write_scene(&read.scene, to, opts)
}

// ─── flat API ───────────────────────────────────────────────────────────────

/// Read any supported container into the flat, world-space view.
pub fn read(bytes: &[u8], from: MeshFormat) -> Result<Mesh, MeshError> {
    check_size(bytes)?;
    let mesh = match from {
        MeshFormat::Glb => glb::read(bytes)?,
        MeshFormat::Gltf => pose::flatten(&glb::read_gltf_scene(bytes, &NoSidecars)?.scene)?,
        MeshFormat::Obj => obj::read(bytes)?,
        MeshFormat::Stl => stl::read(bytes)?,
        MeshFormat::Ply => ply::read(bytes)?,
        MeshFormat::Usdz => pose::flatten(&usd::read_usdz(bytes, &NoSidecars)?.scene)?,
        MeshFormat::Usda => pose::flatten(&usd::read_usda(bytes, &NoSidecars)?.scene)?,
        MeshFormat::Usdc => pose::flatten(&usd::read_usdc(bytes, &NoSidecars)?.scene)?,
        MeshFormat::ThreeMf => threemf::read(bytes)?,
        MeshFormat::Dae => dae::read(bytes)?,
        MeshFormat::Fbx => fbx::read(bytes)?,
    };
    mesh.validate()?;
    if mesh.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    Ok(mesh)
}

/// Write the flat representation as any supported container, as one file.
pub fn write(mesh: &Mesh, to: MeshFormat) -> Result<Vec<u8>, MeshError> {
    mesh.validate()?;
    if mesh.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    match to {
        MeshFormat::Glb => glb::write(mesh),
        MeshFormat::Gltf => Ok(glb::write_gltf_scene(&pose::from_flat(mesh), true)?.files.remove(0).bytes),
        MeshFormat::Obj => obj::write(mesh),
        MeshFormat::Stl => stl::write(mesh),
        MeshFormat::Ply => ply::write(mesh),
        MeshFormat::Usdz | MeshFormat::Usda | MeshFormat::Usdc => {
            Ok(write_scene(&pose::from_flat(mesh), to, WriteOptions { self_contained: true, ..Default::default() })?.package.files.remove(0).bytes)
        }
        MeshFormat::ThreeMf => threemf::write(mesh),
        MeshFormat::Dae => dae::write(mesh),
        MeshFormat::Fbx => fbx::write_scene(&fbx::prepare(&pose::from_flat(mesh), WriteOptions::default())?, 30.0, FbxColorSpace::default(), LightUnits::default()),
    }
}

/// Convert bytes from one container to another, as one file.
///
/// This goes through the scene, so a target that can hold materials, textures,
/// skins or animation keeps them; use [`convert_scene`] to also learn what a
/// poorer target dropped. A same-format conversion is NOT a passthrough — it
/// round-trips through the scene, which keeps one code path (so `glb -> glb`
/// cannot quietly succeed on bytes every other conversion would reject) and
/// normalises whatever the vendor emitted into the subset this module
/// guarantees.
pub fn convert(bytes: &[u8], from: MeshFormat, to: MeshFormat) -> Result<Vec<u8>, MeshError> {
    let mut w = convert_scene(bytes, from, &NoSidecars, to, WriteOptions { self_contained: true, ..Default::default() })?;
    Ok(w.package.files.remove(0).bytes)
}
