//! `source_model`: the mesh a 3D operation works on (PixelGenie 3D doc §3A).
//!
//! An operation (`rig`, `animate`, …) names its input three ways: a litegen
//! generation id, a URL, or base64 bytes. Whichever it is, litegen reads the
//! mesh with its own reader and refuses — before the billed submit — anything
//! the vendor would refuse (§2 rule 6): a container it cannot read or convert,
//! too many bytes or faces, no skin where one is required, no texture where
//! one is required. A source in a container the vendor does not take is
//! converted to one it does. The vendor gets the result through
//! [`SourceInput`], and records how it sent it.

use std::sync::Arc;

use crate::capabilities::ParamSpecSourceModel;
use crate::mesh::{MeshFormat, WriteOptions};

/// Largest source litegen reads when the model declares no `max_bytes`.
pub const MAX_SOURCE_BYTES: u64 = 256 * 1024 * 1024;

/// What litegen's own reader found in a source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default, serde::Serialize)]
pub struct SourceFacts {
    pub triangles: u64,
    pub skinned: bool,
    pub textured: bool,
}

/// A source ready for the vendor.
#[derive(Debug, Clone)]
pub struct PreparedSource {
    /// In a container the vendor takes.
    pub bytes: Vec<u8>,
    /// The container of `bytes`.
    pub format: String,
    /// The container the caller's source arrived in.
    pub received_format: String,
    /// SHA-256 of the source as received: what lineage records.
    pub sha256: String,
    pub facts: SourceFacts,
}

/// Why a source was refused. Every one is decided before the billed submit.
#[derive(Debug, Clone, PartialEq)]
pub enum SourceError {
    /// Not a generation the caller can read, or one with no mesh.
    NotFound(String),
    /// A generation the caller can read that has not finished yet.
    NotReady(String),
    /// A container litegen cannot read, or cannot turn into one the vendor takes.
    FormatUnsupported(String),
    TooLarge { bytes: u64, max: u64 },
    TooManyFaces { faces: u64, max: u64 },
    NotRigged,
    NotTextured,
    /// A URL source could not be fetched.
    Fetch(String),
}

impl SourceError {
    pub fn code(&self) -> &'static str {
        match self {
            SourceError::NotFound(_) => "source_not_found",
            SourceError::NotReady(_) => "source_not_ready",
            SourceError::FormatUnsupported(_) => "source_format_unsupported",
            SourceError::TooLarge { .. } => "source_too_large",
            SourceError::TooManyFaces { .. } => "source_too_many_faces",
            SourceError::NotRigged => "source_not_rigged",
            SourceError::NotTextured => "source_not_textured",
            SourceError::Fetch(_) => "source_unreachable",
        }
    }

    pub fn status(&self) -> u16 {
        match self {
            SourceError::NotFound(_) => 404,
            SourceError::NotReady(_) => 409,
            SourceError::Fetch(_) => 422,
            _ => 422,
        }
    }

    pub fn message(&self) -> String {
        match self {
            SourceError::NotFound(m) | SourceError::NotReady(m) | SourceError::FormatUnsupported(m) | SourceError::Fetch(m) => m.clone(),
            SourceError::TooLarge { bytes, max } => format!("the source is {bytes} bytes; this model takes at most {max}"),
            SourceError::TooManyFaces { faces, max } => format!("the source has {faces} triangles; this model takes at most {max}"),
            SourceError::NotRigged => "this model animates a rigged mesh, and the source has no skin; rig it first".into(),
            SourceError::NotTextured => "this model needs a textured mesh, and the source has no texture".into(),
        }
    }
}

/// Read `bytes` as `format`, check them against what the model takes, and
/// convert them to a container it takes if they are not in one. CPU work:
/// call from a blocking thread.
pub fn prepare(bytes: Vec<u8>, format: &str, spec: &ParamSpecSourceModel) -> Result<PreparedSource, SourceError> {
    let from = MeshFormat::parse(format)
        .ok_or_else(|| SourceError::FormatUnsupported(format!("litegen does not read '{format}' meshes")))?;
    let read = crate::mesh::read_scene(&bytes, from, &crate::mesh::package::NoSidecars)
        .map_err(|e| SourceError::FormatUnsupported(format!("the source could not be read as {format}: {e}")))?;
    let scene = read.scene;
    let facts = SourceFacts {
        triangles: scene.triangle_count() as u64,
        skinned: !scene.skins.is_empty(),
        textured: scene.materials.iter().any(|m| m.texture_refs().next().is_some()),
    };
    if let Some(max) = spec.max_faces {
        if facts.triangles > max {
            return Err(SourceError::TooManyFaces { faces: facts.triangles, max });
        }
    }
    if spec.requires_skin && !facts.skinned {
        return Err(SourceError::NotRigged);
    }
    if spec.requires_texture && !facts.textured {
        return Err(SourceError::NotTextured);
    }
    let sha256 = crate::util::content::sha256_hex(&bytes);
    let accepted = |f: &str| spec.formats.iter().any(|a| a.eq_ignore_ascii_case(f));
    let (out, out_format) = if accepted(from.as_str()) {
        (bytes, from.as_str().to_string())
    } else {
        let mut last = String::from("no accepted container");
        let mut converted = None;
        for target in spec.formats.iter().filter_map(|f| MeshFormat::parse(f)) {
            match crate::mesh::write_scene(&scene, target, WriteOptions::default()) {
                Ok(w) => {
                    if let Some(file) = w.package.files.into_iter().next() {
                        converted = Some((file.bytes, target.as_str().to_string()));
                        break;
                    }
                }
                Err(e) => last = e.to_string(),
            }
        }
        converted.ok_or_else(|| {
            SourceError::FormatUnsupported(format!(
                "this model takes {:?}, and the {} source could not be converted to any of them: {last}",
                spec.formats, from.as_str()
            ))
        })?
    };
    let max_bytes = spec.max_bytes.unwrap_or(MAX_SOURCE_BYTES);
    if out.len() as u64 > max_bytes {
        return Err(SourceError::TooLarge { bytes: out.len() as u64, max: max_bytes });
    }
    Ok(PreparedSource { bytes: out, format: out_format, received_format: from.as_str().to_string(), sha256, facts })
}

/// Bytes of a stored 3D generation's mesh, preferring a container the model
/// takes as it is. `stores` are tried in order (the app's own bucket first).
pub async fn read_generation_source(
    stores: &[Arc<dyn crate::store::ImageStorage>],
    gen: &crate::types::Generation,
    spec: &ParamSpecSourceModel,
) -> Result<(Vec<u8>, String), SourceError> {
    use crate::types::GenerationStatus;
    if gen.media_type != "model3d" {
        return Err(SourceError::NotFound(format!("generation {} is not a 3D generation", gen.id)));
    }
    match gen.status {
        GenerationStatus::Pending | GenerationStatus::Processing => {
            return Err(SourceError::NotReady(format!("generation {} has not finished yet; chain it once it is completed", gen.id)))
        }
        GenerationStatus::Failed | GenerationStatus::Cancelled => {
            return Err(SourceError::NotFound(format!("generation {} {} and has no mesh", gen.id, gen.status)))
        }
        GenerationStatus::Completed => {}
    }
    let meshes: Vec<_> = crate::types::generation::generation_assets(gen).into_iter().filter(|a| a.kind == "mesh").collect();
    let pick = meshes
        .iter()
        .find(|a| spec.formats.iter().any(|f| f.eq_ignore_ascii_case(&a.format)))
        .or_else(|| meshes.iter().find(|a| a.format == crate::types::DEFAULT_MODEL3D_FORMAT))
        .or_else(|| meshes.first())
        .ok_or_else(|| SourceError::NotFound(format!("generation {} has no mesh", gen.id)))?;
    for store in stores {
        let Some(key) = store.key_of(&pick.url) else { continue };
        return match store.get(&key).await {
            Ok(Some((bytes, _))) => Ok((bytes.to_vec(), pick.format.clone())),
            Ok(None) => Err(SourceError::NotFound(format!("generation {}'s mesh is no longer stored", gen.id))),
            Err(e) => Err(SourceError::Fetch(format!("generation {}'s mesh could not be read: {e}", gen.id))),
        };
    }
    Err(SourceError::NotFound(format!("generation {}'s mesh is not held by litegen's storage", gen.id)))
}

/// Download a URL source, refusing private addresses and anything larger than
/// the model takes.
pub async fn fetch_url_source(url: &str, spec: &ParamSpecSourceModel) -> Result<Vec<u8>, SourceError> {
    crate::util::ssrf::validate_public_url(url).await.map_err(SourceError::Fetch)?;
    let max = spec.max_bytes.unwrap_or(MAX_SOURCE_BYTES).min(MAX_SOURCE_BYTES);
    let client = crate::util::ssrf::public_only_download_client(
        std::time::Duration::from_secs(300),
        std::time::Duration::from_secs(60),
    );
    crate::util::ssrf::download_capped(&client, url, max).await.map(|(bytes, _)| bytes).map_err(fetch_error)
}

/// A URL source's download failure, as the caller's answer.
pub fn fetch_error(e: crate::util::ssrf::CappedDownloadError) -> SourceError {
    use crate::util::ssrf::CappedDownloadError;
    match e {
        CappedDownloadError::Transport(why) => SourceError::Fetch(format!("the source could not be fetched: {why}")),
        CappedDownloadError::Status(status) => SourceError::Fetch(format!("the source URL answered HTTP {status}")),
        CappedDownloadError::TooLarge { bytes, max } => SourceError::TooLarge { bytes, max },
    }
}

/// Decode a base64 source (a `data:` prefix is allowed).
pub fn decode_base64_source(value: &str) -> Result<Vec<u8>, SourceError> {
    use base64::Engine;
    let raw = value.split_once(";base64,").map(|(_, b)| b).unwrap_or(value);
    base64::engine::general_purpose::STANDARD
        .decode(raw.trim())
        .map_err(|e| SourceError::FormatUnsupported(format!("source_model.value is not base64: {e}")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::capabilities::ParamSpecSourceModel;
    use crate::mesh::{fixtures, glb, MeshFormat, WriteOptions};

    fn glb_of(scene: &crate::mesh::scene::Scene) -> Vec<u8> {
        glb::write_scene(scene).unwrap()
    }

    fn spec(formats: &[&str]) -> ParamSpecSourceModel {
        ParamSpecSourceModel { formats: formats.iter().map(|f| f.to_string()).collect(), ..Default::default() }
    }

    #[test]
    fn a_source_in_an_accepted_container_reaches_the_vendor_byte_for_byte() {
        let bytes = glb_of(&fixtures::cube());
        let p = prepare(bytes.clone(), "glb", &spec(&["glb", "fbx"])).unwrap();
        assert_eq!(p.bytes, bytes);
        assert_eq!((p.format.as_str(), p.received_format.as_str()), ("glb", "glb"));
        assert_eq!(p.sha256, crate::util::content::sha256_hex(&bytes));
        assert_eq!(p.facts.triangles, fixtures::cube().triangle_count() as u64);
        assert!(!p.facts.skinned && !p.facts.textured);
    }

    #[test]
    fn a_source_in_another_container_is_converted_to_one_the_vendor_takes() {
        let fbx = crate::mesh::convert_scene(&glb_of(&fixtures::cube()), MeshFormat::Glb, &crate::mesh::package::NoSidecars, MeshFormat::Fbx, WriteOptions::default())
            .unwrap()
            .package
            .files
            .remove(0)
            .bytes;
        let p = prepare(fbx.clone(), "fbx", &spec(&["glb"])).unwrap();
        assert_eq!(&p.bytes[..4], b"glTF", "what the vendor takes");
        assert_eq!((p.format.as_str(), p.received_format.as_str()), ("glb", "fbx"));
        assert_eq!(p.sha256, crate::util::content::sha256_hex(&fbx), "lineage names what the caller sent");
    }

    #[test]
    fn what_the_vendor_would_refuse_is_refused_first() {
        let cube = glb_of(&fixtures::cube());
        let too_many = ParamSpecSourceModel { max_faces: Some(2), ..spec(&["glb"]) };
        assert!(matches!(prepare(cube.clone(), "glb", &too_many), Err(SourceError::TooManyFaces { faces: 12, max: 2 })));
        let too_big = ParamSpecSourceModel { max_bytes: Some(16), ..spec(&["glb"]) };
        assert!(matches!(prepare(cube.clone(), "glb", &too_big), Err(SourceError::TooLarge { max: 16, .. })));
        let needs_skin = ParamSpecSourceModel { requires_skin: true, ..spec(&["glb"]) };
        assert!(matches!(prepare(cube.clone(), "glb", &needs_skin), Err(SourceError::NotRigged)));
        let needs_texture = ParamSpecSourceModel { requires_texture: true, ..spec(&["glb"]) };
        assert!(matches!(prepare(cube.clone(), "glb", &needs_texture), Err(SourceError::NotTextured)));
        assert!(matches!(prepare(b"not a mesh".to_vec(), "glb", &spec(&["glb"])), Err(SourceError::FormatUnsupported(_))));
    }

    #[test]
    fn a_rigged_or_textured_source_meets_those_requirements() {
        let skinned = glb_of(&fixtures::skinned());
        let p = prepare(skinned, "glb", &ParamSpecSourceModel { requires_skin: true, ..spec(&["glb"]) }).unwrap();
        assert!(p.facts.skinned);
        let textured = glb_of(&fixtures::textured());
        let p = prepare(textured, "glb", &ParamSpecSourceModel { requires_texture: true, ..spec(&["glb"]) }).unwrap();
        assert!(p.facts.textured);
    }

    #[test]
    fn each_refusal_has_its_documented_code() {
        assert_eq!(SourceError::NotFound("x".into()).code(), "source_not_found");
        assert_eq!(SourceError::NotFound("x".into()).status(), 404);
        assert_eq!(SourceError::NotReady("x".into()).code(), "source_not_ready");
        assert_eq!(SourceError::NotReady("x".into()).status(), 409);
        assert_eq!(SourceError::FormatUnsupported("x".into()).code(), "source_format_unsupported");
        assert_eq!(SourceError::TooLarge { bytes: 2, max: 1 }.code(), "source_too_large");
        assert_eq!(SourceError::TooManyFaces { faces: 2, max: 1 }.code(), "source_too_many_faces");
        assert_eq!(SourceError::NotRigged.code(), "source_not_rigged");
        assert_eq!(SourceError::NotTextured.code(), "source_not_textured");
        assert_eq!(SourceError::NotRigged.status(), 422);
    }
}
