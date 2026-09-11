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
        output_format: None, texture: None, pbr: None, target_polycount: None,
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
