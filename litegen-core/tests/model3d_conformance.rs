//! The `model3d` family's contract, asserted across every 3D row the shipped
//! catalog advertises — vendor rows and mocks alike.
//!
//! `catalog_conformance.rs` carries the original structural checks; this file
//! holds the ones that read the CURRENT spelling of the format param
//! (`output_formats`, via `output_formats_spec`) and the API's served view of
//! it. Its older glb check matches only the superseded scalar `output_format`,
//! which no shipped row uses any more, so on today's catalog it never runs.

use litegen::capabilities::{CapabilityRegistry, MediaType, ModelSchema, ParamSpec};
use litegen::proxy::model3d_convert::{served_output_formats_spec, vendor_jobs_needed};
use litegen::types::{Model3dFormat, DEFAULT_MODEL3D_FORMAT};

fn catalog() -> CapabilityRegistry {
    let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop();
    dir.push("models");
    CapabilityRegistry::from_dir(&dir).expect("the shipped catalog loads")
}

fn model3d(reg: &CapabilityRegistry) -> Vec<&ModelSchema> {
    let rows: Vec<&ModelSchema> = reg.all().filter(|m| m.media_type == MediaType::Model3d).collect();
    assert!(!rows.is_empty(), "the catalog advertises no 3D models");
    rows
}

#[test]
fn every_3d_model_that_declares_formats_can_emit_glb() {
    // GLB is the only container every 3D vendor emits, the conversion source
    // for the rest, and what a caller gets when they name nothing.
    let reg = catalog();
    for m in model3d(&reg) {
        if let Some(spec) = m.output_formats_spec() {
            assert!(
                spec.enum_values.iter().any(|f| f == DEFAULT_MODEL3D_FORMAT),
                "{}: output_formats {:?} has no glb",
                m.id, spec.enum_values
            );
        }
        assert!(
            !m.resolve_output_formats(None).is_empty(),
            "{}: an omitted output_formats must still promise something",
            m.id
        );
    }
}

#[test]
fn every_declared_container_is_in_the_3d_vocabulary() {
    let reg = catalog();
    for m in model3d(&reg) {
        let Some(spec) = m.output_formats_spec() else { continue };
        for f in spec.enum_values.iter().chain(&spec.default) {
            assert!(Model3dFormat::parse(f).is_some(), "{}: '{f}' is not a Model3dFormat", m.id);
        }
    }
}

#[test]
fn every_default_format_set_is_satisfiable_by_its_own_model() {
    // A default that needed more vendor jobs than max_items would make every
    // request that omits the param fail validation.
    let reg = catalog();
    for m in model3d(&reg) {
        let Some(spec) = m.output_formats_spec() else { continue };
        let default = m.resolve_output_formats(None);
        if let Some(max) = spec.max_items {
            assert!(
                vendor_jobs_needed(&default) <= max,
                "{}: default {default:?} needs {} vendor jobs but max_items is {max}",
                m.id, vendor_jobs_needed(&default)
            );
        }
    }
}

#[test]
fn the_served_format_set_is_a_superset_of_what_the_vendor_emits() {
    // What GET /v1/models/{id} offers a caller must include everything the
    // vendor itself produces, and only add containers litegen derives.
    let reg = catalog();
    for m in model3d(&reg) {
        let Some(native) = m.output_formats_spec() else { continue };
        let served = served_output_formats_spec(&native);
        for f in &native.enum_values {
            assert!(served.enum_values.contains(f), "{}: served set dropped native '{f}'", m.id);
        }
        for f in served.enum_values.iter().filter(|f| !native.enum_values.contains(f)) {
            assert!(served.derivable_values.contains(f), "{}: '{f}' is served but neither native nor derivable", m.id);
        }
        assert_eq!(served.max_items, native.max_items, "{}: max_items is the vendor-job budget, unchanged", m.id);
    }
}

#[test]
fn every_vendor_3d_model_names_its_api_reference() {
    // The URL a maintainer re-reads when a field mapping is questioned — and
    // the one the dashboard links to.
    let reg = catalog();
    for m in model3d(&reg).into_iter().filter(|m| m.provider != "mock") {
        let url = m.docs_url.as_deref().unwrap_or_else(|| panic!("{} has no docs_url", m.id));
        assert!(url.starts_with("https://"), "{}: docs_url {url} is not https", m.id);
    }
}

#[test]
fn a_model_without_a_prompt_field_never_requires_one() {
    let reg = catalog();
    for m in model3d(&reg) {
        if m.prompt.max_length == Some(0) {
            assert!(!m.prompt.required, "{}: requires a prompt it cannot take", m.id);
            assert!(!m.capabilities.text_to_3d, "{}: text-to-3D with no prompt field", m.id);
        }
    }
}

#[test]
fn image_only_models_require_the_image_they_cannot_work_without() {
    let reg = catalog();
    for m in model3d(&reg) {
        if m.capabilities.text_to_3d || !m.capabilities.image_to_3d {
            continue;
        }
        let init = m
            .ref_inputs
            .as_ref()
            .and_then(|r| r.roles.get("init"))
            .unwrap_or_else(|| panic!("{}: image-only with no init role", m.id));
        assert!(init.required && init.min_count >= 1, "{}: image-only but init is optional", m.id);
    }
}

#[test]
fn capability_flags_agree_with_the_declared_params() {
    // `supports_pbr` / `supports_rig` / `supports_texture` are read off the
    // declared Bool params; a non-bool kind would advertise a knob the
    // validator refuses.
    let reg = catalog();
    for m in model3d(&reg) {
        for p in ["pbr", "rig", "texture"] {
            if let Some(spec) = m.params.get(p) {
                assert!(matches!(spec, ParamSpec::Bool(_)), "{}: {p} must be kind: bool", m.id);
            }
        }
    }
}
