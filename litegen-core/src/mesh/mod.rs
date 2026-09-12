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
//! # Scope, and why it stops where it does
//!
//! Supported: **glb, obj, stl, ply** — every pair, both directions. These are
//! the containers whose geometry can be read and written completely, in-process,
//! with no external tool and no licence attached.
//!
//! Deliberately absent:
//!
//! - **fbx** — the only complete writer is Autodesk's licence-encumbered SDK.
//!   Open writers produce files that fail to load in the DCC tools that are the
//!   entire reason someone wants FBX.
//! - **usdz** — a zip of USD crates, needing USD tooling to produce. For its
//!   main use (iOS AR Quick Look) `<model-viewer>` already generates it in the
//!   browser from the GLB.
//!
//! A format that cannot be produced correctly is not produced. The alternative
//! — emitting a plausible file that breaks downstream — is worse than the
//! honest error, because it surfaces at the customer rather than at us.
//!
//! # What conversion loses
//!
//! Lossiness here is a property of the target container, not a shortcoming of
//! this code, and callers must be told rather than surprised:
//!
//! | target | keeps | drops |
//! |--------|-------|-------|
//! | glb    | positions, indices, normals, uvs, base colour | — |
//! | obj    | positions, indices, normals, uvs | colour (needs an `.mtl` sidecar) |
//! | ply    | positions, indices, normals, vertex colour | uvs |
//! | stl    | positions, facet normals | indices, uvs, colour — STL has no concept of any of them |
//!
//! Textures are out of scope entirely in v1: every texture-bearing target needs
//! sidecar files, and litegen's asset rehosting keys each file independently, so
//! a sidecar reference inside a mesh would not resolve. See
//! `proxy::router::rehost_model3d_files`.
//!
//! # Isolation
//!
//! This module imports NOTHING from the rest of litegen — no types, no
//! providers, no storage. It is bytes in, bytes out, and `mesh_isolation_tests`
//! enforces that mechanically so it stays directly unit-testable (and
//! extractable into its own crate) as the codebase moves around it.

pub mod ir;

pub mod glb;
pub mod obj;
pub mod ply;
pub mod stl;

#[cfg(test)]
mod tests;
#[cfg(test)]
mod matrix_tests;
#[cfg(test)]
mod isolation_tests;

pub use ir::Mesh;

use std::fmt;

/// A container this module can read and write.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub enum MeshFormat {
    Glb,
    Obj,
    Stl,
    Ply,
}

impl MeshFormat {
    /// Every format, in a stable order. The conversion matrix iterates this, so
    /// adding a variant automatically adds a row and a column of coverage.
    pub const ALL: [MeshFormat; 4] =
        [MeshFormat::Glb, MeshFormat::Obj, MeshFormat::Stl, MeshFormat::Ply];

    /// The lowercase extension, without a dot — the same spelling litegen uses
    /// in `Model3dAsset.format` and in storage keys.
    pub fn as_str(self) -> &'static str {
        match self {
            MeshFormat::Glb => "glb",
            MeshFormat::Obj => "obj",
            MeshFormat::Stl => "stl",
            MeshFormat::Ply => "ply",
        }
    }

    /// Parse an extension. Case-insensitive; `None` for anything this module
    /// cannot handle, which callers must treat as "not convertible" rather than
    /// falling back to some default.
    pub fn parse(s: &str) -> Option<MeshFormat> {
        match s.trim().to_ascii_lowercase().as_str() {
            "glb" => Some(MeshFormat::Glb),
            "obj" => Some(MeshFormat::Obj),
            "stl" => Some(MeshFormat::Stl),
            "ply" => Some(MeshFormat::Ply),
            _ => None,
        }
    }

    /// Whether this container carries per-vertex texture coordinates.
    pub fn keeps_uvs(self) -> bool {
        matches!(self, MeshFormat::Glb | MeshFormat::Obj)
    }

    /// Whether this container carries any colour at all without a sidecar.
    pub fn keeps_color(self) -> bool {
        matches!(self, MeshFormat::Glb | MeshFormat::Ply)
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

/// Read any supported container into the neutral representation.
pub fn read(bytes: &[u8], from: MeshFormat) -> Result<Mesh, MeshError> {
    if bytes.len() > MAX_INPUT_BYTES {
        return Err(MeshError::Unsupported(format!(
            "input is {} bytes, over the {MAX_INPUT_BYTES}-byte limit",
            bytes.len()
        )));
    }
    let mesh = match from {
        MeshFormat::Glb => glb::read(bytes)?,
        MeshFormat::Obj => obj::read(bytes)?,
        MeshFormat::Stl => stl::read(bytes)?,
        MeshFormat::Ply => ply::read(bytes)?,
    };
    mesh.validate()?;
    if mesh.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    Ok(mesh)
}

/// Write the neutral representation as any supported container.
pub fn write(mesh: &Mesh, to: MeshFormat) -> Result<Vec<u8>, MeshError> {
    mesh.validate()?;
    if mesh.triangle_count() == 0 {
        return Err(MeshError::Empty);
    }
    match to {
        MeshFormat::Glb => glb::write(mesh),
        MeshFormat::Obj => obj::write(mesh),
        MeshFormat::Stl => stl::write(mesh),
        MeshFormat::Ply => ply::write(mesh),
    }
}

/// Convert bytes from one container to another.
///
/// A same-format conversion is NOT a passthrough — it round-trips through the
/// IR. That keeps one code path (so `glb -> glb` cannot quietly succeed on
/// bytes every other conversion would reject) and normalises whatever the
/// vendor emitted into the subset this module guarantees.
pub fn convert(bytes: &[u8], from: MeshFormat, to: MeshFormat) -> Result<Vec<u8>, MeshError> {
    write(&read(bytes, from)?, to)
}
