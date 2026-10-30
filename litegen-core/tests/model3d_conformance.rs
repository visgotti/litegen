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
        // Views drive a multiview model without `init` (validator: no
        // references at all is still refused, there being no text arm).
        if m.capabilities.text_to_3d || m.capabilities.multiview_to_3d || !m.capabilities.image_to_3d {
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

// ─── Rigging rows (PixelGenie 3D doc §3B) ───────────────────────────────────

fn source_spec(m: &ModelSchema) -> &litegen::capabilities::ParamSpecSourceModel {
    match m.params.get("source_model") {
        Some(ParamSpec::SourceModel(s)) => s,
        other => panic!("{}: source_model is {other:?}", m.id),
    }
}

fn string_enum(m: &ModelSchema, param: &str) -> Vec<String> {
    match m.params.get(param) {
        Some(ParamSpec::String(s)) => s.enum_values.clone(),
        other => panic!("{}: {param} is {other:?}", m.id),
    }
}

/// @see <https://developers.tripo3d.ai/en/docs/animations-rig> (2026-10-30):
/// input "GLB, FBX, OBJ, STL" up to "150 MB"; rig_type biped … aquatic,
/// others; spec tripo, mixamo, actorcore, unity, unreal, vrm; out_format glb,
/// fbx. @see <https://developers.tripo3d.ai/en/pricing>: "Auto Rig / 25",
/// "1 credit = $0.01 USD".
#[test]
fn tripo_rig_takes_what_tripos_rig_endpoint_accepts() {
    let reg = catalog();
    let m = reg.get("tripo/rig").expect("tripo/rig");
    assert_eq!(m.task, litegen::capabilities::ModelTask::Rig);
    let src = source_spec(m);
    assert_eq!(src.formats, ["glb", "fbx", "obj", "stl"]);
    assert_eq!(src.max_bytes, Some(150_000_000));
    assert_eq!(string_enum(m, "skeleton"), ["humanoid", "quadruped", "hexapod", "octopod", "avian", "serpentine", "aquatic", "other"]);
    assert_eq!(string_enum(m, "joint_naming"), ["vendor", "vrm", "mixamo", "unity", "unreal"]);
    assert!(!m.params.contains_key("height_m"), "Tripo's rig takes no height");
    let formats = m.output_formats_spec().unwrap();
    assert_eq!((formats.enum_values.as_slice(), formats.max_items), (["glb".to_string(), "fbx".to_string()].as_slice(), Some(1)));
    assert!((m.pricing.base_cost_usd - 0.25).abs() < 1e-9);
    assert_eq!(m.prompt.max_length, Some(0), "the rig endpoint has no prompt");
    assert!(m.docs_url.as_deref().is_some_and(|u| u.ends_with("/animations-rig")));
}

/// @see <https://fal.ai/api/openapi/queue/openapi.json?endpoint_id=fal-ai/meshy/rigging>
/// (2026-10-30): model_url (GLB), height_meters; Meshy's limits
/// (docs.meshy.ai/en/api/rigging): "100 MB as a data URI", "at most 320,000
/// faces", untextured meshes unsuitable. Price: "$0.20 per request".
#[test]
fn fal_meshy_rig_takes_a_textured_glb_within_meshys_limits() {
    let reg = catalog();
    let m = reg.get("fal/meshy-rig").expect("fal/meshy-rig");
    assert_eq!(m.task, litegen::capabilities::ModelTask::Rig);
    let src = source_spec(m);
    assert_eq!(src.formats, ["glb"]);
    assert_eq!((src.max_bytes, src.max_faces, src.requires_texture), (Some(100_000_000), Some(320_000), true));
    assert!(matches!(m.params.get("height_m"), Some(ParamSpec::Float(_))));
    assert!(!m.params.contains_key("skeleton") && !m.params.contains_key("joint_naming"), "Meshy rigs humanoids with its own names");
    assert!((m.pricing.base_cost_usd - 0.20).abs() < 1e-9, "no animation is sent, so none is billed");
}

// ─── Animation rows (PixelGenie 3D doc §3C) ─────────────────────────────────

fn animation_spec(m: &ModelSchema) -> &litegen::capabilities::ParamSpecAnimationList {
    match m.params.get("animations") {
        Some(ParamSpec::AnimationList(a)) => a,
        other => panic!("{}: animations is {other:?}", m.id),
    }
}

/// @see <https://developers.tripo3d.ai/en/docs/animations-retarget> (2026-10-30):
/// input "The returned task_id of the rigged model."; animations "up to 5";
/// the v2.5/v3.0 presets; `animate_in_place`. Price: "Animation Retarget / 10
/// / animation".
#[test]
fn tripo_animate_offers_the_retarget_presets_on_tripos_own_rig() {
    let reg = catalog();
    let m = reg.get("tripo/animate").expect("tripo/animate");
    assert_eq!(m.task, litegen::capabilities::ModelTask::Animate);
    assert!(source_spec(m).requires_skin);
    let a = animation_spec(m);
    assert_eq!(a.max_items, 5);
    assert_eq!(a.in_place, litegen::capabilities::InPlaceMode::Native, "animate_in_place");
    assert!(a.text_to_motion.is_none(), "Tripo documents no text-to-motion");
    let ids: Vec<&str> = a.presets.iter().map(|p| p.id.as_str()).collect();
    assert_eq!(ids.len(), 16);
    for id in ["preset:idle", "preset:walk", "preset:slash", "preset:quadruped:walk", "preset:aquatic:march"] {
        assert!(ids.contains(&id), "{id}");
    }
    assert!((m.pricing.base_cost_usd - 0.10).abs() < 1e-9);
    assert_eq!(m.native_orientation.as_ref().map(|o| o.front.as_str()), Some("source"));
}

/// @see <https://docs.meshy.ai/en/api/animation> (2026-10-30): action_ids "1
/// to 10 … with no repeats"; no in-place flag; post_process change_fps.
/// @see <https://docs.meshy.ai/en/api/rigging>: GLB, "100 MB as a data URI",
/// "at most 320,000 faces"; animation_type biped | quadruped (OpenAPI).
#[test]
fn meshy_rig_and_animate_take_what_meshys_endpoints_accept() {
    let reg = catalog();
    let rig = reg.get("meshy/rig").expect("meshy/rig");
    let src = source_spec(rig);
    assert_eq!((src.formats.as_slice(), src.max_bytes, src.max_faces, src.requires_texture), (["glb".to_string()].as_slice(), Some(100_000_000), Some(320_000), true));
    assert_eq!(string_enum(rig, "skeleton"), ["humanoid", "quadruped"]);
    assert!(matches!(rig.params.get("height_m"), Some(ParamSpec::Float(_))));
    let anim = reg.get("meshy/animate").expect("meshy/animate");
    let a = animation_spec(anim);
    assert_eq!(a.max_items, 10);
    assert_eq!(a.in_place, litegen::capabilities::InPlaceMode::Litegen, "Meshy has no in-place flag");
    assert!(a.presets.iter().all(|p| p.id.parse::<u32>().is_ok() && p.label.is_some()), "library ids, with Meshy's names");
    for (id, canonical) in [("0", "idle"), ("4", "attack_melee"), ("8", "death")] {
        let p = a.presets.iter().find(|p| p.id == id).unwrap();
        assert_eq!(p.canonical.as_deref(), Some(canonical), "{id}");
    }
}

#[test]
fn every_animate_model_keeps_its_sources_orientation_or_proves_its_own() {
    // An animate model hands back the rig it was given; claiming fixed axes
    // for it would be a guess about the caller's mesh.
    let reg = catalog();
    for m in model3d(&reg).into_iter().filter(|m| m.task == litegen::capabilities::ModelTask::Animate) {
        let o = m.native_orientation.as_ref().unwrap_or_else(|| panic!("{}: no native_orientation", m.id));
        assert_eq!((o.up.as_str(), o.front.as_str()), ("source", "source"), "{}", m.id);
    }
}

/// 3D doc §3F: a texture model paints from a prompt, a `style` image or both,
/// so it takes at least one; and texture, remesh and segment models keep
/// their source's axes (they work on the caller's mesh), as animate models do.
#[test]
fn texture_remesh_and_segment_models_take_what_they_work_from_and_keep_its_axes() {
    use litegen::capabilities::ModelTask;
    let reg = catalog();
    let ops: Vec<&ModelSchema> = model3d(&reg)
        .into_iter()
        .filter(|m| matches!(m.task, ModelTask::Texture | ModelTask::Remesh | ModelTask::Segment))
        .collect();
    assert!(ops.len() >= 3, "a mock for each operation");
    for m in ops {
        let o = m.native_orientation.as_ref().unwrap_or_else(|| panic!("{}: no native_orientation", m.id));
        assert_eq!((o.up.as_str(), o.front.as_str()), ("source", "source"), "{}", m.id);
        if m.task == ModelTask::Texture {
            let takes_prompt = m.prompt.max_length != Some(0);
            let takes_style = m.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("style"));
            assert!(takes_prompt || takes_style, "{}: paints from nothing", m.id);
        }
    }
}
