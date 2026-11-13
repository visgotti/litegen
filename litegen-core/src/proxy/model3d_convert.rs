//! Which mesh containers litegen can deliver, and what that costs a vendor.
//!
//! `output_formats` means the same thing on every model: what the vendor
//! emits natively plus every container litegen can derive from a file it can
//! read. This module answers the questions that promise raises before a job
//! is submitted — what is deliverable (`deliverable_formats`), what the
//! catalog serves (`served_output_formats_spec`), how many vendor jobs a
//! request needs (`vendor_jobs_needed`) and which container to ask a vendor
//! for (`prefers_vendor_native`). The deriving itself happens at delivery
//! (`proxy::model3d_deliver`), in [`crate::mesh`], which knows nothing about
//! litegen.
//!
//! Every container in `Model3dFormat` is derivable: `mesh` reads and writes
//! glb, gltf, obj, stl, ply, 3mf, dae, fbx, usdz, usda and usdc, each verified
//! against independent readers (`docs/superpowers/3d-format-conversion.md`).
//! What a target cannot hold is reported by `mesh` as losses, not faked.

use crate::mesh::MeshFormat;

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

/// [`prefers_vendor_native`] as the caller's `format_source` has it: `native`
/// (the default) is that rule; `glb` asks the vendor only for what litegen
/// cannot derive, so a promised fbx or usdz is made here from the vendor's
/// GLB, which carries the most, and the GLB is never itself derived from a
/// thinner container.
pub fn asks_vendor_for(format: &str, source: Option<crate::types::Model3dFormatSource>) -> bool {
    match source.unwrap_or_default() {
        crate::types::Model3dFormatSource::Native => prefers_vendor_native(format),
        crate::types::Model3dFormatSource::Glb => !is_derivable(format),
    }
}

/// The mesh library's FBX colour space for a request's `fbx_color_space`.
pub fn fbx_color_space(choice: Option<crate::types::Model3dFbxColorSpace>) -> crate::mesh::FbxColorSpace {
    match choice.unwrap_or_default() {
        crate::types::Model3dFbxColorSpace::Srgb => crate::mesh::FbxColorSpace::Srgb,
        crate::types::Model3dFbxColorSpace::Linear => crate::mesh::FbxColorSpace::Linear,
    }
}

/// The mesh library's light units for a request's `fbx_light_units` or
/// `usd_light_units`.
pub fn light_units(choice: Option<crate::types::Model3dLightUnits>) -> crate::mesh::LightUnits {
    match choice.unwrap_or_default() {
        crate::types::Model3dLightUnits::Photometric => crate::mesh::LightUnits::Photometric,
        crate::types::Model3dLightUnits::Blender => crate::mesh::LightUnits::Blender,
    }
}

/// The mesh library's Collada morph option for a request's `dae_morphs`.
pub fn dae_morphs(choice: Option<crate::types::Model3dDaeMorphs>) -> crate::mesh::DaeMorphs {
    match choice.unwrap_or_default() {
        crate::types::Model3dDaeMorphs::Bake => crate::mesh::DaeMorphs::Bake,
        crate::types::Model3dDaeMorphs::Keep => crate::mesh::DaeMorphs::Keep,
    }
}

/// The mesh library's USDZ root option for a request's `usdz_root`.
pub fn usdz_root(choice: Option<crate::types::Model3dUsdzRoot>) -> crate::mesh::UsdzRoot {
    match choice.unwrap_or_default() {
        crate::types::Model3dUsdzRoot::Usda => crate::mesh::UsdzRoot::Usda,
        crate::types::Model3dUsdzRoot::Usdc => crate::mesh::UsdzRoot::Usdc,
    }
}

/// The mesh library's USD clips option for a request's `usd_clips`: `None`
/// when none was asked, which is each container's own default.
pub fn usd_clips(choice: Option<crate::types::Model3dUsdClips>) -> Option<crate::mesh::UsdClips> {
    choice.map(|c| match c {
        crate::types::Model3dUsdClips::All => crate::mesh::UsdClips::All,
        crate::types::Model3dUsdClips::First => crate::mesh::UsdClips::First,
    })
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
#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deliverable_is_native_plus_what_we_can_derive() {
        // The GLB-only vendor — fal, Replicate, Stability. Four deliverables
        // from a catalogue entry that declares one.
        let d = deliverable_formats(&["glb".to_string()]);
        for f in ["glb", "gltf", "obj", "stl", "ply", "3mf", "dae", "fbx", "usdz", "usda", "usdc"] {
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
        for f in ["glb", "gltf", "obj", "stl", "ply", "3mf", "dae", "fbx", "usdz", "usda", "usdc"] {
            assert!(is_derivable(f), "{f}");
        }
        // `.usd` names either encoding, so it is no container of its own.
        assert!(!is_derivable("blend") && !is_derivable("usd") && !is_derivable("png"));
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
        for f in ["3mf", "dae", "usda", "usdc", "gltf"] {
            assert_eq!(prefers_vendor_native(f), !is_derivable(f), "{f}: from the vendor exactly when we cannot make it");
        }
        for f in ["glb", "obj", "stl", "ply"] {
            assert!(!prefers_vendor_native(f), "{f} derives losslessly enough from glb");
        }
    }
}
