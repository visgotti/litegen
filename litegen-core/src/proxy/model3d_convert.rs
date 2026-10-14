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
//! Every container in `Model3dFormat` is derivable: `mesh` reads and writes
//! glb, gltf, obj, stl, ply, 3mf, dae, fbx, usdz and usda, each verified
//! against independent readers (`docs/superpowers/3d-format-conversion.md`).
//! What a target cannot hold is reported by `mesh` as losses, not faked.

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

/// Whether to ask the vendor for `format` itself whenever it offers it, rather
/// than deriving it here from another container.
///
/// Every container litegen cannot derive, obviously. Beyond that, fbx and usdz:
/// a vendor exports them from a DCC-grade pipeline (rigs, its own materials,
/// extra takes), which a conversion from its glb does not reproduce, though
/// `mesh` writes both. A derivable format that is also preferred-native still
/// counts as derivable for the vendor-job budget; this only decides which
/// container the one vendor job is spent on, and the other requested
/// containers are then derived from what the vendor returned.
pub fn prefers_vendor_native(format: &str) -> bool {
    !is_derivable(format) || matches!(format.trim().to_ascii_lowercase().as_str(), "fbx" | "usdz")
}

/// Everything a model can actually deliver: what the vendor emits natively,
/// plus what we can derive from it.
///
/// Derivation needs a readable source, so a model whose native set is entirely
/// unreadable by us (fbx and usdz only, say) gains nothing.
pub fn deliverable_formats(native: &[String]) -> Vec<String> {
    let mut out: Vec<String> = native.to_vec();
    if native.iter().any(|n| is_derivable(n)) {
        for d in derivable_formats() {
            if !out.iter().any(|o| o.eq_ignore_ascii_case(&d)) {
                out.push(d);
            }
        }
    }
    out
}

/// The `output_formats` spec as a CALLER reads it — what `GET /v1/models/{id}`
/// serves in place of the catalog's own.
///
/// The catalog declares what the vendor emits; a client rendering `params`
/// needs what it may ask for, which is that plus everything derivable from it
/// — the exact set the validator accepts. `max_items` is left alone: it still
/// counts vendor jobs, and `derivable_values` names the members that share
/// one, so a client can apply [`vendor_jobs_needed`]'s budget rule without
/// knowing anything about mesh conversion.
pub fn served_output_formats_spec(
    native: &crate::capabilities::ParamSpecStringArray,
) -> crate::capabilities::ParamSpecStringArray {
    let derivable_values = if native.enum_values.iter().any(|v| is_derivable(v)) {
        derivable_formats()
    } else {
        Vec::new()
    };
    crate::capabilities::ParamSpecStringArray {
        enum_values: deliverable_formats(&native.enum_values),
        derivable_values,
        ..native.clone()
    }
}

/// How many containers the VENDOR must produce to satisfy this request.
///
/// This is what `max_items` actually constrains. Every requested format we can
/// derive locally comes out of one readable source, so a whole set of them
/// costs exactly one vendor job; a format we cannot derive (none of
/// `Model3dFormat`'s containers today; Sloyd's .blend, say) has to come from
/// the vendor itself and costs one each.
///
/// The pleasing consequence: Rodin, whose `geometry_file_format` is a scalar
/// and so has `max_items: 1`, serves `[glb, obj, stl, ply]` from a single GLB,
/// and `[glb, fbx]` from a single FBX: its own (`prefers_vendor_native`), with
/// the glb derived from it.
pub fn vendor_jobs_needed(requested: &[String]) -> usize {
    let native_only = requested.iter().filter(|r| !is_derivable(r)).count();
    let needs_source = requested.iter().any(|r| is_derivable(r));
    native_only + usize::from(needs_source)
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
/// preferable to ours: it comes straight out of the vendor's pipeline, where
/// ours is a conversion of it.
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

    // Convert from GLB when we have it: glTF is `mesh`'s own scene model, so
    // reading it loses nothing. Otherwise take the first readable mesh.
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
        // only a container we cannot read, such as .blend — in both cases the
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
            // Anything outside the converter's scope (.blend, say).
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
        // The vendor's own export comes out of its own pipeline, where ours
        // would be a conversion of it, so it always wins.
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
            &["glb".into(), "blend".into(), "abc".into()],
        );
        assert_eq!(formats_of(&out), ["glb"], "no invented containers");
        let named: Vec<&str> = report.failures.iter().map(|(f, _)| f.as_str()).collect();
        assert!(named.contains(&"blend") && named.contains(&"abc"), "{:?}", report.failures);
    }

    #[test]
    fn every_container_derives_from_a_glb_and_reads_back() {
        // fbx and usdz included: the vendor's own is preferred when it offers
        // them, but a GLB-only vendor can still deliver them.
        let requested = derivable_formats();
        let (out, report) = synthesize_missing_formats(vec![glb_file()], &requested);
        assert!(report.failures.is_empty(), "{:?}", report.failures);
        for f in out.iter().filter(|f| f.kind == Model3dAssetKind::Mesh) {
            let fmt = MeshFormat::parse(&f.format).expect("a derivable format");
            let back = mesh::read(&f.bytes, fmt).unwrap_or_else(|e| panic!("{}: {e}", f.format));
            assert_eq!(back.triangle_count(), crate::providers::model3d::glb::CUBE_TRIANGLE_COUNT as usize, "{}: the cube", f.format);
        }
        assert_eq!(formats_of(&out).len(), requested.len());
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
    fn deliverable_is_native_plus_what_we_can_derive() {
        // The GLB-only vendor — fal, Replicate, Stability. Four deliverables
        // from a catalogue entry that declares one.
        let d = deliverable_formats(&["glb".to_string()]);
        for f in ["glb", "gltf", "obj", "stl", "ply", "3mf", "dae", "fbx", "usdz", "usda"] {
            assert!(d.iter().any(|x| x == f), "{f} missing from {d:?}");
        }
        assert!(!d.iter().any(|x| x == "blend"), "nothing outside the converter: {d:?}");
    }

    #[test]
    fn a_native_set_we_cannot_read_gains_nothing() {
        // Derivation needs a readable SOURCE. A model emitting only containers
        // this module cannot parse has nothing to convert from, and claiming
        // otherwise would advertise formats that could never be delivered.
        let d = deliverable_formats(&["blend".to_string(), "abc".to_string()]);
        assert_eq!(d, vec!["blend".to_string(), "abc".to_string()]);
    }

    #[test]
    fn native_formats_are_never_duplicated_by_derivation() {
        let d = deliverable_formats(&["glb".to_string(), "obj".to_string()]);
        let mut sorted = d.clone();
        sorted.sort();
        sorted.dedup();
        assert_eq!(sorted.len(), d.len(), "{d:?}");
    }

    #[test]
    fn vendor_jobs_counts_one_source_for_the_whole_derivable_set() {
        // This is what `max_items` actually constrains, and why a
        // one-format-per-job vendor like Rodin can serve four containers.
        assert_eq!(vendor_jobs_needed(&["glb".into()]), 1);
        assert_eq!(
            vendor_jobs_needed(&["glb".into(), "obj".into(), "stl".into(), "ply".into()]),
            1,
            "derivable formats all come out of one readable source",
        );
        // fbx and usdz derive too: Rodin's [glb, fbx] is its own fbx plus a
        // glb derived from it, one job.
        assert_eq!(vendor_jobs_needed(&["glb".into(), "fbx".into(), "usdz".into()]), 1);
        // A container we cannot derive has to come from the vendor itself.
        assert_eq!(vendor_jobs_needed(&["blend".into()]), 1);
        assert_eq!(vendor_jobs_needed(&["glb".into(), "blend".into()]), 2);
        assert_eq!(vendor_jobs_needed(&["blend".into(), "abc".into()]), 2);
        assert_eq!(
            vendor_jobs_needed(&["glb".into(), "obj".into(), "blend".into(), "abc".into()]),
            3,
            "two native-only formats plus one source for the derivable pair",
        );
    }

    #[test]
    fn derivable_covers_exactly_what_the_mesh_module_supports() {
        for f in ["glb", "gltf", "obj", "stl", "ply", "3mf", "dae", "fbx", "usdz", "usda"] {
            assert!(is_derivable(f), "{f}");
        }
        assert!(!is_derivable("blend") && !is_derivable("usdc") && !is_derivable("png"));
        assert_eq!(derivable_formats().len(), MeshFormat::ALL.len());
    }

    #[test]
    fn the_served_spec_is_the_callers_set_with_its_shared_job_named() {
        // Rodin: a scalar vendor field (one job), five native containers.
        // Expectations are computed from the converter's own set, so this holds
        // whichever containers `mesh` can write.
        let native = crate::capabilities::ParamSpecStringArray {
            enum_values: ["glb", "usdz", "fbx", "obj", "stl"].map(String::from).to_vec(),
            max_items: Some(1),
            default: vec!["glb".into()],
            label: Some("Output formats".into()),
            ..Default::default()
        };
        let served = served_output_formats_spec(&native);
        assert_eq!(served.enum_values[..native.enum_values.len()], native.enum_values[..], "native first, in order");
        assert_eq!(served.enum_values, deliverable_formats(&native.enum_values), "then every derived addition");
        assert_eq!(served.derivable_values, derivable_formats());
        assert!(served.enum_values.iter().any(|f| f == "ply"), "a container Rodin cannot emit is offered");
        // Still the VENDOR-job budget, not a count of deliverables.
        assert_eq!(served.max_items, Some(1));
        assert_eq!(served.default, ["glb"]);
        assert_eq!(served.label.as_deref(), Some("Output formats"));
    }

    #[test]
    fn nothing_is_derivable_without_a_readable_source() {
        // A vendor emitting only containers the converter cannot read (Sloyd's
        // .blend, say) gives it nothing to convert from.
        let native = crate::capabilities::ParamSpecStringArray {
            enum_values: vec!["blend".into()],
            max_items: Some(1),
            ..Default::default()
        };
        let served = served_output_formats_spec(&native);
        assert_eq!(served.enum_values, ["blend"]);
        assert!(served.derivable_values.is_empty());
    }

    #[test]
    fn the_served_budget_rule_agrees_with_the_validators() {
        // A client applying the documented rule to `derivable_values` must
        // reach the same verdict as `vendor_jobs_needed`, for every subset of
        // a realistic deliverable set.
        let served = served_output_formats_spec(&crate::capabilities::ParamSpecStringArray {
            enum_values: ["glb", "usdz", "fbx", "obj", "stl"].map(String::from).to_vec(),
            max_items: Some(1),
            ..Default::default()
        });
        let all = served.enum_values.clone();
        for mask in 1u32..(1 << all.len()) {
            let sel: Vec<String> = all.iter().enumerate()
                .filter(|(i, _)| mask & (1 << i) != 0)
                .map(|(_, f)| f.clone())
                .collect();
            let shared = sel.iter().filter(|f| served.derivable_values.contains(f)).count();
            let client_rule = (sel.len() - shared) + usize::from(shared > 0);
            assert_eq!(client_rule, vendor_jobs_needed(&sel), "{sel:?}");
        }
    }

    #[test]
    fn fbx_and_usdz_are_asked_of_the_vendor_whether_or_not_we_can_derive_them() {
        assert!(prefers_vendor_native("fbx") && prefers_vendor_native("USDZ"));
        for f in ["3mf", "dae", "usda", "gltf"] {
            assert_eq!(prefers_vendor_native(f), !is_derivable(f), "{f}: from the vendor exactly when we cannot make it");
        }
        for f in ["glb", "obj", "stl", "ply"] {
            assert!(!prefers_vendor_native(f), "{f} derives losslessly enough from glb");
        }
    }
}
