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
//! Ten containers, every pair, both directions: **glb, gltf, obj, stl, ply,
//! 3mf, dae, fbx, usdz, usda**. These are what 3D vendors deliver and what
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
//! | target | keeps | drops, each reported |
//! |--------|-------|----------------------|
//! | glb, gltf | everything the scene holds | — |
//! | fbx | hierarchy, normals, 8 uv sets, vertex colour, Phong materials (Blender's PBR mapping) with embedded textures, skins, morph targets, the first animation | other clips, texture filters, texture transforms (baked into the uvs), and what `fbx::losses` names: double-sided, alpha mask, unlit, mirrored wrap, a tint beside a texture |
//! | usdz, usda | hierarchy, normals, 2 uv sets, vertex colour, PBR materials and textures (usda: sidecars, package only), skins, morph targets, one animation | other clips, unlit, tangents, texture filters (transforms baked into the uvs) |
//! | dae | hierarchy, normals, 2 uv sets, vertex colour, Phong materials and textures (sidecars, package only), skins, every animation clip | morph targets (applied at their weights: three.js and assimp fail on any `<morph>`), texture filters and transforms |
//! | 3mf | placements (instancing), 1 uv set, vertex colour, base colour and base-colour texture | normals, skins, morph targets and animation (posed at rest), other material properties |
//! | obj | named parts, normals, 1 uv set, vertex colour, Phong materials and textures through an MTL (package only) | hierarchy (flattened), skins, morphs and animation (posed at rest); as one file, materials become vertex colours and textures go |
//! | ply | one mesh, normals, vertex colour | uvs, textures (a flat colour becomes vertex colour), hierarchy, animation |
//! | stl | one mesh of triangles | everything else (facet normals are derived) |
//!
//! # One file, or a package
//!
//! OBJ's MTL and images, glTF's buffers and images, and DAE's and USDA's
//! images live beside the model. `WriteOptions::self_contained` chooses:
//!
//! - **One file** (the default, and what `convert` and the proxy deliver)
//!   embeds where the container can: glb, gltf as data URIs, fbx, usdz and
//!   3mf. Where it cannot, the loss is reported. litegen re-hosts each output
//!   file on its own (`proxy::router::rehost_model3d_files`), so a sidecar
//!   reference inside a delivered mesh would not resolve.
//! - **A package** writes the sidecars too, as files of the returned
//!   `Package`.
//!
//! Readers resolve sidecars through `package::Sidecars`.
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

pub mod ir;
pub mod math;
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
    /// USDA; USDA and USDC roots read.
    Usdz,
    /// A standalone USD layer: USDA text written, USDA or USDC crate read.
    /// Textures are sidecar files through [`write_scene`] with
    /// `self_contained: false`, absent otherwise.
    Usda,
}

impl MeshFormat {
    /// Every format, in a stable order. The conversion matrix iterates this, so
    /// adding a variant automatically adds a row and a column of coverage.
    pub const ALL: [MeshFormat; 10] = [
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
            // `.usd` and `.usdc` stay unparsed: they READ through the USDA
            // reader (a crate is one layer like a text one), but nothing here
            // writes a crate, and a format name must mean both directions.
            "usda" => Some(MeshFormat::Usda),
            "3mf" => Some(MeshFormat::ThreeMf),
            "dae" => Some(MeshFormat::Dae),
            "fbx" => Some(MeshFormat::Fbx),
            _ => None,
        }
    }

    /// What this container can hold — one row of the lossiness table.
    ///
    /// `opts` matters for the containers whose richest form is several files:
    /// a self-contained OBJ has no `.mtl` to put materials in, for instance.
    pub fn capabilities(self, opts: WriteOptions) -> Capabilities {
        match self {
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
                ..Capabilities::none()
            },
            MeshFormat::Obj => Capabilities {
                normals: true,
                uv_sets: 1,
                vertex_colors: true,
                materials: MaterialSupport::Phong,
                textures: true,
                multiple_meshes: true,
                ..Capabilities::none()
            },
            MeshFormat::Ply => Capabilities {
                normals: true,
                vertex_colors: true,
                materials: MaterialSupport::VertexColor,
                ..Capabilities::none()
            },
            MeshFormat::Stl => Capabilities::none(),
            MeshFormat::Usdz | MeshFormat::Usda => usd::capabilities(self, opts),
            // Placed parts with per-triangle colour: base materials, colour
            // groups and one texture (V-flipped coordinates, wrap modes, an
            // auto/linear/nearest filter). No normals, nothing that moves.
            MeshFormat::Dae => dae::capabilities(opts),
            MeshFormat::Fbx => fbx::capabilities(),
            MeshFormat::ThreeMf => Capabilities {
                hierarchy: true,
                uv_sets: 1,
                vertex_colors: true,
                materials: MaterialSupport::ColorOnly,
                textures: true,
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
        // which no capability flag describes.
        if self == MeshFormat::ThreeMf {
            return threemf::prepare(scene);
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
        let caps = self.capabilities(opts);
        if matches!(self, MeshFormat::Usdz | MeshFormat::Usda) {
            // USD's writer encodes less than the generic degrade describes:
            // one timeline, linear keys, PNG/JPEG, joints-only skeletons.
            return usd::prepare(scene, &caps);
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
    /// The input carries no triangles. Always an error: a zero-triangle mesh is
    /// never a useful deliverable, and passing it on turns a vendor failure
    /// into a file the caller has to diagnose themselves.
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
            MeshError::Empty => f.write_str("mesh contains no triangles"),
        }
    }
}

impl std::error::Error for MeshError {}

/// Largest input this module will parse, in bytes.
///
/// Conversion is CPU and memory work on the poll path. A 300k-triangle mesh is
/// roughly 15 MB as GLB and several times that once expanded to a soup, so the
/// cap is what keeps one pathological vendor response from evicting everything
/// else. Callers hitting it should decline the conversion, not retry.
pub const MAX_INPUT_BYTES: usize = 128 * 1024 * 1024;

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
}

impl Default for WriteOptions {
    fn default() -> Self {
        WriteOptions { self_contained: true }
    }
}

/// A written model and everything the target container could not hold.
#[derive(Debug, Clone)]
pub struct SceneWrite {
    pub package: Package,
    pub losses: Vec<Loss>,
}

/// Read any supported container into a scene.
pub fn read_scene(bytes: &[u8], from: MeshFormat, sidecars: &dyn Sidecars) -> Result<SceneRead, MeshError> {
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
        MeshFormat::Stl => (pose::from_flat(&stl::read(bytes)?), Vec::new()),
        MeshFormat::Ply => (ply::read_scene(bytes)?, Vec::new()),
        MeshFormat::Usdz => {
            let r = usd::read_usdz(bytes, sidecars)?;
            (r.scene, r.warnings)
        }
        MeshFormat::Usda => {
            let r = usd::read_usda(bytes, sidecars)?;
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
            let r = fbx::read_scene(bytes, sidecars)?;
            (r.scene, r.warnings)
        }
    };
    scene.validate()?;
    if scene.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    Ok(SceneRead { scene, warnings })
}

/// Write a scene as any supported container, reporting what it cannot hold.
pub fn write_scene(scene: &Scene, to: MeshFormat, opts: WriteOptions) -> Result<SceneWrite, MeshError> {
    scene.validate()?;
    if scene.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    let mut losses = scene::losses(scene, &to.capabilities(opts), to.as_str());
    if to == MeshFormat::ThreeMf {
        losses.extend(threemf::losses(scene));
    }
    if to == MeshFormat::Dae {
        losses.extend(dae::extra_losses(scene, opts));
    }
    if to == MeshFormat::Fbx {
        // What FBX's material and sampler model cannot say, beyond the
        // capability table.
        losses.extend(fbx::losses(scene));
    }
    if matches!(to, MeshFormat::Usdz | MeshFormat::Usda) {
        losses.extend(usd::losses(scene, to.as_str()));
    }
    let prepared = to.prepare(scene, opts)?;
    let package = match to {
        MeshFormat::Glb => Package::single("model.glb", glb::write_scene(&prepared)?),
        MeshFormat::Gltf => glb::write_gltf_scene(&prepared, opts.self_contained)?,
        MeshFormat::Obj => obj::write_scene(&prepared, opts.self_contained)?,
        MeshFormat::Stl => Package::single("model.stl", stl::write(&pose::flatten(&prepared)?)?),
        MeshFormat::Ply => Package::single("model.ply", ply::write_scene(&prepared)?),
        MeshFormat::Usdz => usd::write_usdz(&prepared)?,
        MeshFormat::Usda => usd::write_usda(&prepared, opts)?,
        MeshFormat::ThreeMf => threemf::write_scene(&prepared)?,
        MeshFormat::Dae => dae::write_scene(&prepared, opts)?,
        // Textures are always embedded: FBX's own convention, and the only
        // form every FBX reader resolves without the original directory.
        MeshFormat::Fbx => Package::single("model.fbx", fbx::write_scene(&prepared)?),
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
    let read = read_scene(bytes, from, sidecars)?;
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
        MeshFormat::Usdz | MeshFormat::Usda => {
            Ok(write_scene(&pose::from_flat(mesh), to, WriteOptions { self_contained: true })?.package.files.remove(0).bytes)
        }
        MeshFormat::ThreeMf => threemf::write(mesh),
        MeshFormat::Dae => dae::write(mesh),
        MeshFormat::Fbx => fbx::write_scene(&fbx::prepare(&pose::from_flat(mesh), WriteOptions::default())?),
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
    let mut w = convert_scene(bytes, from, &NoSidecars, to, WriteOptions { self_contained: true })?;
    Ok(w.package.files.remove(0).bytes)
}
