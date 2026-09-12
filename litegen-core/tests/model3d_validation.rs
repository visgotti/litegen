//! Validation surface for the `model3d` family. The lax-drop behaviour here is
//! the single most load-bearing part of the contract aipix codes against: it is
//! what lets a client send a superset of params without knowing which model
//! supports which knob.

use litegen::api::middleware::validator::validate_model3d;
use litegen::capabilities::*;
use litegen::types::*;
// Both globs export a `MediaType` and a `ModelPricing` (capabilities::schema
// and types each define their own). `ModelSchema` needs the capabilities pair;
// naming them explicitly resolves the rustc `ambiguous_glob_imports`
// future-incompat warning, which is slated to become a hard error.
use litegen::capabilities::{MediaType, ModelPricing};
use std::collections::HashMap;

fn schema_with(params: HashMap<String, ParamSpec>) -> ModelSchema {
    ModelSchema {
        id: "mock/mesh-3d".into(),
        provider: "mock".into(),
        media_type: MediaType::Model3d,
        display_name: "Mock Mesh 3D".into(),
        description: String::new(),
        pricing: ModelPricing { base_cost_usd: 0.0, variable_pricing: None },
        capabilities: ModelCapabilityFlags { text_to_3d: true, ..Default::default() },
        prompt: PromptSpec { required: true, min_length: None, max_length: None },
        params,
        ref_inputs: None,
        extra_allowlist: vec![],
        tags: vec![],
    }
}

fn req(strict: bool) -> Model3dGenerationRequest {
    Model3dGenerationRequest {
        base: BaseGenerationRequest {
            prompt: "a low-poly fox".into(),
            model: "mock/mesh-3d".into(),
            n: 1,
            negative_prompt: None,
            seed: None,
            reference_images: vec![],
            strict,
            extra: None,
            metadata: None,
        },
        output_formats: None, output_format: None,
        texture: None, pbr: None, target_polycount: None,
        symmetry: None, topology: None, rig: None,
    }
}

fn int_param(min: i64, max: i64) -> ParamSpec {
    ParamSpec::Int(ParamSpecInt { min: Some(min), max: Some(max), default: None, label: None, description: None })
}

fn str_param(values: &[&str]) -> ParamSpec {
    ParamSpec::String(ParamSpecString {
        max_length: None,
        enum_values: values.iter().map(|s| s.to_string()).collect(),
        pattern: None,
        default: None,
        label: None,
        description: None,
    })
}

fn bool_param() -> ParamSpec {
    ParamSpec::Bool(ParamSpecBool { default: None, label: None, description: None })
}

fn formats_param(values: &[&str], max_items: Option<usize>) -> ParamSpec {
    ParamSpec::StringArray(ParamSpecStringArray {
        enum_values: values.iter().map(|s| s.to_string()).collect(),
        max_items,
        default: vec!["glb".to_string()],
        label: None,
        description: None,
    })
}

// ─── output_formats ─────────────────────────────────────────────────────────
//
// This param is the only one whose validated value becomes a PROMISE: whatever
// survives here is re-checked at all three observers of completion, and a
// generation that delivers less than this fails. So an unsatisfiable request
// has to be rejected here, before the vendor is billed.

#[test]
fn a_format_the_model_cannot_emit_is_rejected_in_both_modes() {
    let schema = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb", "obj"], Some(2))),
    ]));
    for strict in [true, false] {
        let mut r = req(strict);
        r.output_formats = Some(vec!["glb".into(), "fbx".into()]);
        let err = validate_model3d(&schema, r).unwrap_err();
        assert_eq!(err.code, "param_enum_mismatch", "strict={strict}");
        assert_eq!(err.param.as_deref(), Some("output_formats"));
        // A declared param with an unsupported VALUE is a client error in both
        // modes — dropping it in lax mode would silently downgrade the promise.
        assert!(err.message.contains("fbx"), "the error must name the format: {}", err.message);
    }
}

#[test]
fn asking_for_more_containers_than_the_model_emits_is_rejected() {
    // Rodin's shape: `geometry_file_format` is a scalar, so only one container
    // is reachable per job however many the model can produce overall.
    let schema = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb", "obj", "stl"], Some(1))),
    ]));
    let mut r = req(true);
    r.output_formats = Some(vec!["glb".into(), "obj".into()]);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_too_many");
    assert_eq!(err.param.as_deref(), Some("output_formats"));
}

#[test]
fn an_explicitly_empty_list_is_rejected_rather_than_read_as_unset() {
    let schema = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb"], Some(1))),
    ]));
    let mut r = req(true);
    r.output_formats = Some(vec![]);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_out_of_range");
    assert_eq!(err.param.as_deref(), Some("output_formats"));
}

#[test]
fn the_superseded_scalar_spelling_folds_into_the_plural() {
    let schema = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb", "obj"], Some(2))),
    ]));
    let mut r = req(true);
    r.output_format = Some("obj".into());
    let out = validate_model3d(&schema, r).unwrap();
    assert_eq!(out.request.output_formats.as_deref(), Some(&["obj".to_string()][..]));
    assert!(out.request.output_format.is_none(), "the scalar must not survive alongside the plural");
    assert!(out.dropped.is_empty(), "folding is not dropping: {:?}", out.dropped);
}

#[test]
fn sending_both_spellings_keeps_the_plural_and_reports_the_scalar_dropped() {
    let schema = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb", "obj"], Some(2))),
    ]));
    let mut r = req(true);
    r.output_formats = Some(vec!["glb".into()]);
    r.output_format = Some("obj".into());
    let out = validate_model3d(&schema, r).unwrap();
    assert_eq!(out.request.output_formats.as_deref(), Some(&["glb".to_string()][..]));
    // Silently discarding one of two conflicting values is how a caller ends up
    // convinced they asked for something they did not.
    assert!(out.dropped.contains(&"output_format".to_string()), "{:?}", out.dropped);
}

#[test]
fn formats_are_case_folded_and_deduplicated() {
    let schema = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb", "obj"], Some(2))),
    ]));
    let mut r = req(true);
    // `max_items` is 2 and this names three entries — it must still pass,
    // because it is one container asked for twice plus one more.
    r.output_formats = Some(vec!["GLB".into(), "glb".into(), " obj ".into()]);
    let out = validate_model3d(&schema, r).unwrap();
    assert_eq!(
        out.request.output_formats.as_deref(),
        Some(&["glb".to_string(), "obj".to_string()][..]),
    );
}

#[test]
fn a_legacy_scalar_spec_still_validates_the_plural_request() {
    // A catalog that has not migrated yet: the model declares the old scalar
    // `output_format`, and a client sends the new plural field.
    let schema = schema_with(HashMap::from([
        ("output_format".to_string(), str_param(&["glb", "obj"])),
    ]));
    let mut r = req(true);
    r.output_formats = Some(vec!["obj".into()]);
    let out = validate_model3d(&schema, r).unwrap();
    assert_eq!(out.request.output_formats.as_deref(), Some(&["obj".to_string()][..]));

    // ...and the scalar spec means one container per job, so two is too many.
    let mut r = req(true);
    r.output_formats = Some(vec!["glb".into(), "obj".into()]);
    assert_eq!(validate_model3d(&schema, r).unwrap_err().code, "param_too_many");
}

#[test]
fn a_model_with_no_format_spec_drops_in_lax_and_errors_in_strict() {
    let schema = schema_with(HashMap::new());

    let mut r = req(false);
    r.output_formats = Some(vec!["obj".into()]);
    let out = validate_model3d(&schema, r).unwrap();
    assert!(out.request.output_formats.is_none());
    assert!(out.dropped.contains(&"output_formats".to_string()), "{:?}", out.dropped);

    let mut r = req(true);
    r.output_formats = Some(vec!["obj".into()]);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_unsupported");
    assert_eq!(err.param.as_deref(), Some("output_formats"));
}

#[test]
fn resolution_never_yields_an_empty_promise() {
    // Whatever the model declares and the caller omits, something must be
    // promised — an empty set would make the completion guard a no-op.
    let declared = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb", "obj"], Some(2))),
    ]));
    assert_eq!(declared.resolve_output_formats(None), vec!["glb".to_string()]);
    assert_eq!(
        declared.resolve_output_formats(Some(&["OBJ".to_string()])),
        vec!["obj".to_string()],
    );

    let undeclared = schema_with(HashMap::new());
    assert_eq!(undeclared.resolve_output_formats(None), vec!["glb".to_string()]);
    assert_eq!(undeclared.resolve_output_formats(Some(&[])), vec!["glb".to_string()]);
}

#[test]
fn lax_mode_drops_unsupported_params_instead_of_erroring() {
    let schema = schema_with(HashMap::new()); // model supports nothing
    let mut r = req(false);
    r.target_polycount = Some(5000);
    r.pbr = Some(true);
    r.rig = Some(true);
    r.topology = Some("quad".into());

    let out = validate_model3d(&schema, r).expect("lax must never error on unsupported params");
    assert_eq!(out.request.target_polycount, None, "dropped params are cleared from the request");
    assert_eq!(out.request.pbr, None);
    assert_eq!(out.request.rig, None);
    assert_eq!(out.request.topology, None);
    for p in ["target_polycount", "pbr", "rig", "topology"] {
        assert!(out.dropped.contains(&p.to_string()), "'{p}' must be reported as dropped: {:?}", out.dropped);
    }
}

#[test]
fn strict_mode_rejects_an_unsupported_param() {
    let schema = schema_with(HashMap::new());
    let mut r = req(true);
    r.target_polycount = Some(5000);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_unsupported");
    assert_eq!(err.param.as_deref(), Some("target_polycount"));
}

#[test]
fn supported_params_pass_through_untouched() {
    let schema = schema_with(HashMap::from([
        ("target_polycount".to_string(), int_param(100, 300_000)),
        ("topology".to_string(), str_param(&["triangle", "quad"])),
        ("output_format".to_string(), str_param(&["glb", "obj"])),
        ("texture".to_string(), bool_param()),
        ("pbr".to_string(), bool_param()),
        ("rig".to_string(), bool_param()),
        ("symmetry".to_string(), str_param(&["off", "auto", "on"])),
    ]));
    let mut r = req(true);
    r.target_polycount = Some(5000);
    r.topology = Some("quad".into());
    r.output_format = Some("obj".into());
    r.texture = Some(true);
    r.pbr = Some(false);
    r.rig = Some(true);
    r.symmetry = Some("auto".into());

    let out = validate_model3d(&schema, r).unwrap();
    assert!(out.dropped.is_empty(), "nothing should drop: {:?}", out.dropped);
    assert_eq!(out.request.target_polycount, Some(5000));
    assert_eq!(out.request.topology.as_deref(), Some("quad"));
    assert_eq!(out.request.symmetry.as_deref(), Some("auto"));
}

#[test]
fn out_of_range_polycount_is_rejected_even_in_lax_mode() {
    // A declared param with a bad VALUE is a client error in both modes — only
    // UNSUPPORTED params get the lax drop.
    let schema = schema_with(HashMap::from([
        ("target_polycount".to_string(), int_param(100, 300_000)),
    ]));
    let mut r = req(false);
    r.target_polycount = Some(9_000_000);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_out_of_range");
    assert_eq!(err.param.as_deref(), Some("target_polycount"));
}

#[test]
fn enum_mismatch_on_topology_is_rejected() {
    let schema = schema_with(HashMap::from([
        ("topology".to_string(), str_param(&["triangle", "quad"])),
    ]));
    let mut r = req(true);
    r.topology = Some("voxel".into());
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_enum_mismatch");
}

#[test]
fn multiview_roles_are_accepted_when_the_model_declares_them() {
    // Mode is inferred, not passed: `view-*` refs mean multiview-to-3D.
    let mut schema = schema_with(HashMap::new());
    schema.ref_inputs = Some(RefInputSpec {
        max_total: 4,
        default_role: Some("init".into()),
        provider_format: RefProviderFormat::Base64,
        roles: HashMap::from([
            ("init".to_string(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
            ("view-front".to_string(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
            ("view-back".to_string(), RefRoleSpec { required: false, min_count: 0, max_count: 1 }),
        ]),
    });
    let mut r = req(true);
    r.base.reference_images = vec![
        ReferenceImage { kind: RefImageKind::Url, value: "https://x/f.png".into(), role: Some("view-front".into()) },
        ReferenceImage { kind: RefImageKind::Url, value: "https://x/b.png".into(), role: Some("view-back".into()) },
    ];
    let out = validate_model3d(&schema, r).unwrap();
    assert_eq!(out.request.base.reference_images.len(), 2);
    assert!(out.dropped.is_empty());
}

#[test]
fn prompt_is_still_required() {
    let schema = schema_with(HashMap::new());
    let mut r = req(true);
    r.base.prompt = "   ".into();
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "prompt_required");
}
