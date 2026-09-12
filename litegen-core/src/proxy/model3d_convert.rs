//! Filling in mesh containers the vendor did not return.
//!
//! Sits between a provider's poll result and asset rehosting: the vendor hands
//! back whatever containers it produces, and this synthesises the rest locally
//! so `output_formats` means the same thing on every model. Without it, the
//! param is only as good as the vendor — and the models litegen is most likely
//! to ship first (fal- and Replicate-hosted ones, Stability) emit GLB and
//! nothing else, so `output_formats` on those could only ever be `["glb"]`.
//!
//! The conversion itself lives in [`crate::mesh`], which knows nothing about
//! litegen. This module is the seam: it maps `Model3dFile`s on and off that
//! pure byte-in/byte-out API and decides *what* to convert, never *how*.
//!
//! What it will not do: invent fbx or usdz. Neither can be written correctly
//! in-process — FBX's only complete writer is licence-encumbered, USDZ needs
//! USD tooling — and a plausible-but-broken file is worse than an honest
//! failure, because it surfaces at the customer instead of at us.

use crate::mesh::{self, MeshFormat};
use crate::providers::Model3dFile;
use crate::types::{Model3dAssetKind, DEFAULT_MODEL3D_FORMAT};

/// Containers litegen can produce locally from any other one it can read.
pub fn derivable_formats() -> Vec<String> {
    MeshFormat::ALL.iter().map(|f| f.as_str().to_string()).collect()
}

/// Whether `format` can be synthesised locally given some mesh we can read.
pub fn is_derivable(format: &str) -> bool {
    MeshFormat::parse(format).is_some()
}

/// What happened while filling in the missing containers.
///
/// Conversion failures do NOT fail the generation here. The completion guard
/// already fails a generation whose promised containers are absent, and it is
/// the single place that decision belongs — duplicating it would give two code
/// paths that can disagree. What this carries is the *cause*, so the operator
/// sees "the vendor's glb would not parse" in the log rather than inferring it
/// from a generic "format not returned" on the caller's side.
#[derive(Debug, Default)]
pub struct SynthesisReport {
    pub synthesized: Vec<String>,
    pub failures: Vec<(String, String)>,
}

/// Add a mesh file for every requested container that is missing and derivable.
///
/// Returns the original files plus whatever could be synthesised. Formats that
/// are already present are left alone — a vendor's own export is always
/// preferable to ours, because it can carry material and texture detail this
/// module's geometry-only IR does not model.
///
/// CPU-bound: parsing and re-encoding a 300k-triangle mesh is real work on a
/// path that also holds a poll tick, so callers must run this on a blocking
/// thread rather than inline on the async runtime.
pub fn synthesize_missing_formats(
    files: Vec<Model3dFile>,
    requested: &[String],
) -> (Vec<Model3dFile>, SynthesisReport) {
    let mut report = SynthesisReport::default();

    let have: Vec<String> = files
        .iter()
        .filter(|f| f.kind == Model3dAssetKind::Mesh)
        .map(|f| f.format.to_ascii_lowercase())
        .collect();

    let missing: Vec<&String> = requested
        .iter()
        .filter(|r| !have.iter().any(|h| h.eq_ignore_ascii_case(r)))
        .collect();
    if missing.is_empty() {
        return (files, report);
    }

    // Convert from GLB when we have it: it is the only container in the set
    // that carries UVs AND colour, so using anything else as the source would
    // discard detail we were holding. Otherwise take the first readable mesh.
    let source = files
        .iter()
        .filter(|f| f.kind == Model3dAssetKind::Mesh)
        .find(|f| f.format.eq_ignore_ascii_case(DEFAULT_MODEL3D_FORMAT))
        .or_else(|| {
            files
                .iter()
                .filter(|f| f.kind == Model3dAssetKind::Mesh)
                .find(|f| MeshFormat::parse(&f.format).is_some())
        });

    let Some(source) = source else {
        // Nothing readable to convert FROM. Not an error here: either the
        // vendor returned no mesh at all (the mesh guard's case) or it returned
        // only a container we cannot read, such as fbx — in both cases the
        // completion guard produces the caller-facing failure.
        for m in &missing {
            report.failures.push(((*m).clone(), "no readable mesh to convert from".into()));
        }
        return (files, report);
    };

    let from = match MeshFormat::parse(&source.format) {
        Some(f) => f,
        None => return (files, report),
    };

    let mut out = files.clone();
    for want in missing {
        let Some(to) = MeshFormat::parse(want) else {
            // fbx, usdz, or anything else outside the module's scope.
            report.failures.push((want.clone(), "format cannot be produced locally".into()));
            continue;
        };
        match mesh::convert(&source.bytes, from, to) {
            Ok(bytes) => {
                out.push(Model3dFile {
                    kind: Model3dAssetKind::Mesh,
                    format: to.as_str().to_string(),
                    content_type: crate::proxy::storage::model3d_content_type(
                        to.as_str(),
                        "application/octet-stream",
                    )
                    .to_string(),
                    bytes,
                    // Converted geometry has the same triangle count as its
                    // source, so the vendor's number still describes it.
                    polycount: source.polycount,
                    width: None,
                    height: None,
                });
                report.synthesized.push(to.as_str().to_string());
            }
            Err(e) => report.failures.push((to.as_str().to_string(), e.to_string())),
        }
    }

    (out, report)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn glb_file() -> Model3dFile {
        Model3dFile {
            kind: Model3dAssetKind::Mesh,
            format: "glb".into(),
            content_type: "model/gltf-binary".into(),
            bytes: crate::providers::model3d::glb::generate_cube_glb("a fox"),
            polycount: Some(12),
            width: None,
            height: None,
        }
    }

    fn preview_file() -> Model3dFile {
        Model3dFile {
            kind: Model3dAssetKind::Preview,
            format: "png".into(),
            content_type: "image/png".into(),
            bytes: vec![0x89, b'P', b'N', b'G'],
            polycount: None,
            width: Some(8),
            height: Some(8),
        }
    }

    fn formats_of(files: &[Model3dFile]) -> Vec<String> {
        let mut v: Vec<String> = files
            .iter()
            .filter(|f| f.kind == Model3dAssetKind::Mesh)
            .map(|f| f.format.clone())
            .collect();
        v.sort();
        v
    }

    #[test]
    fn a_glb_only_vendor_can_still_satisfy_every_derivable_format() {
        // The case this module exists for: fal-, Replicate- and
        // Stability-hosted models return GLB and nothing else.
        let (out, report) = synthesize_missing_formats(
            vec![glb_file(), preview_file()],
            &["glb".into(), "obj".into(), "stl".into(), "ply".into()],
        );
        assert_eq!(formats_of(&out), ["glb", "obj", "ply", "stl"]);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        assert_eq!(report.synthesized.len(), 3);
        // Non-mesh assets pass through untouched.
        assert_eq!(out.iter().filter(|f| f.kind == Model3dAssetKind::Preview).count(), 1);
    }

    #[test]
    fn a_format_the_vendor_already_returned_is_never_replaced() {
        // The vendor's own export can carry material and texture detail our
        // geometry-only IR does not model, so it always wins.
        let mut vendor_obj = glb_file();
        vendor_obj.format = "obj".into();
        vendor_obj.bytes = b"# the vendor's own obj\nv 0 0 0\nv 1 0 0\nv 0 1 0\nf 1 2 3\n".to_vec();

        let (out, report) = synthesize_missing_formats(
            vec![glb_file(), vendor_obj.clone()],
            &["glb".into(), "obj".into()],
        );
        assert!(report.synthesized.is_empty(), "nothing needed synthesising");
        let obj = out.iter().find(|f| f.format == "obj").unwrap();
        assert_eq!(obj.bytes, vendor_obj.bytes, "the vendor's obj must survive verbatim");
    }

    #[test]
    fn formats_we_cannot_write_are_reported_rather_than_faked() {
        let (out, report) = synthesize_missing_formats(
            vec![glb_file()],
            &["glb".into(), "fbx".into(), "usdz".into()],
        );
        assert_eq!(formats_of(&out), ["glb"], "no invented containers");
        let named: Vec<&str> = report.failures.iter().map(|(f, _)| f.as_str()).collect();
        assert!(named.contains(&"fbx") && named.contains(&"usdz"), "{:?}", report.failures);
    }

    #[test]
    fn an_unreadable_source_fails_loudly_instead_of_emitting_garbage() {
        let mut broken = glb_file();
        broken.bytes.truncate(20);
        let (out, report) = synthesize_missing_formats(vec![broken], &["glb".into(), "obj".into()]);
        assert_eq!(formats_of(&out), ["glb"]);
        assert_eq!(report.failures.len(), 1);
        assert_eq!(report.failures[0].0, "obj");
        assert!(!report.failures[0].1.is_empty(), "the cause must reach the operator's log");
    }

    #[test]
    fn nothing_happens_when_every_requested_format_is_present() {
        let files = vec![glb_file(), preview_file()];
        let (out, report) = synthesize_missing_formats(files.clone(), &["glb".into()]);
        assert_eq!(out.len(), files.len());
        assert!(report.synthesized.is_empty() && report.failures.is_empty());
    }

    #[test]
    fn a_result_with_no_mesh_at_all_is_left_for_the_mesh_guard() {
        let (out, report) = synthesize_missing_formats(vec![preview_file()], &["glb".into()]);
        assert!(formats_of(&out).is_empty());
        assert_eq!(report.failures.len(), 1, "the reason is recorded, the verdict is not ours");
    }

    #[test]
    fn matching_is_case_insensitive_on_the_vendors_spelling() {
        // `format` is free-form vendor text; "GLB" is a spelling difference,
        // not a second container to go and synthesise.
        let mut shouty = glb_file();
        shouty.format = "GLB".into();
        let (out, report) = synthesize_missing_formats(vec![shouty], &["glb".into()]);
        assert_eq!(formats_of(&out), ["GLB"]);
        assert!(report.synthesized.is_empty() && report.failures.is_empty());
    }

    #[test]
    fn derivable_covers_exactly_what_the_mesh_module_supports() {
        assert!(is_derivable("glb") && is_derivable("obj") && is_derivable("stl") && is_derivable("ply"));
        assert!(!is_derivable("fbx") && !is_derivable("usdz") && !is_derivable("png"));
        assert_eq!(derivable_formats().len(), MeshFormat::ALL.len());
    }
}
