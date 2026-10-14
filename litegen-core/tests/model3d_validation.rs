//! Validation surface for the `model3d` family. The lax-drop behaviour here is
//! the single most load-bearing part of the contract aipix codes against: it is
//! what lets a client send a superset of params without knowing which model
//! supports which knob.

use litegen::api::middleware::validator::validate_model3d;
use litegen::proxy::model3d_convert::is_derivable;
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
        docs_url: None,
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
        ..Default::default()
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
        // Neither emitted nor derivable: Sloyd's .blend, say.
        r.output_formats = Some(vec!["glb".into(), "blend".into()]);
        let err = validate_model3d(&schema, r).unwrap_err();
        assert_eq!(err.code, "param_enum_mismatch", "strict={strict}");
        assert_eq!(err.param.as_deref(), Some("output_formats"));
        // A declared param with an unsupported VALUE is a client error in both
        // modes — dropping it in lax mode would silently downgrade the promise.
        assert!(err.message.contains("blend"), "the error must name the format: {}", err.message);
    }
}

#[test]
fn max_items_bounds_what_the_provider_must_produce_not_what_may_be_asked_for() {
    // Rodin's shape: `geometry_file_format` is a scalar, so it emits exactly one
    // container per job. That no longer caps the CALLER, because obj/stl/ply are
    // derived locally from the one glb — which is the whole point of local
    // conversion. Four deliverables, one vendor job.
    let schema = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb"], Some(1))),
    ]));
    let mut r = req(true);
    r.output_formats = Some(vec!["glb".into(), "obj".into(), "stl".into(), "ply".into()]);
    let out = validate_model3d(&schema, r).expect("derivable formats cost one vendor job");
    assert_eq!(out.request.output_formats.as_deref().unwrap().len(), 4);

    // But a container we CANNOT derive has to come from the vendor itself, so
    // glb + blend genuinely is two jobs and a scalar-field vendor cannot do it.
    let schema = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb", "blend"], Some(1))),
    ]));
    let mut r = req(true);
    r.output_formats = Some(vec!["glb".into(), "blend".into()]);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_too_many");
    assert_eq!(err.param.as_deref(), Some("output_formats"));
}

#[test]
fn a_derivable_format_is_accepted_even_when_the_model_does_not_emit_it() {
    // The GLB-only vendor case — fal, Replicate, Stability. Without this,
    // `output_formats` on exactly those models could only ever mean ["glb"].
    let schema = schema_with(HashMap::from([
        ("output_formats".to_string(), formats_param(&["glb"], None)),
    ]));
    let mut r = req(true);
    r.output_formats = Some(vec!["stl".into()]);
    let out = validate_model3d(&schema, r).expect("stl is derivable from glb");
    assert_eq!(out.request.output_formats.as_deref(), Some(&["stl".to_string()][..]));

    // A format nobody can produce is still refused, and the message lists what
    // is actually deliverable rather than only what the vendor emits.
    let mut r = req(true);
    r.output_formats = Some(vec!["blend".into()]);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_enum_mismatch");
    assert!(err.message.contains("stl"), "must advertise the derivable set: {}", err.message);
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

    // ...and the scalar spec means the VENDOR emits one container per job. glb
    // plus obj is still one job, because the obj is derived from the glb.
    let mut r = req(true);
    r.output_formats = Some(vec!["glb".into(), "obj".into()]);
    assert!(validate_model3d(&schema, r).is_ok(), "one vendor job, two deliverables");
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

// ─── Against the shipped catalog ────────────────────────────────────────────
//
// The synthetic schemas above prove the mechanics. These prove the catalog a
// deployment actually serves: every canonical 3D param, refused or dropped on
// a real model that does not declare it and honoured on one that does, plus
// the vendor limits the fal rows encode. A catalog edit that loosens any of
// them fails here, before it can be billed.

fn catalog() -> CapabilityRegistry {
    let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    dir.pop();
    dir.push("models");
    CapabilityRegistry::from_dir(&dir).expect("the shipped catalog loads")
}

fn real(id: &str) -> ModelSchema {
    catalog().get(id).unwrap_or_else(|| panic!("{id} is not in the shipped catalog")).clone()
}

fn image_ref() -> ReferenceImage {
    ReferenceImage { kind: RefImageKind::Url, value: "https://example.com/robot.png".into(), role: Some("init".into()) }
}

/// A request a given real model accepts as-is: an image for the image-only
/// models, a prompt for the rest.
fn valid_for(schema: &ModelSchema, strict: bool) -> Model3dGenerationRequest {
    let mut r = req(strict);
    r.base.model = schema.id.clone();
    if schema.capabilities.text_to_3d {
        r.base.prompt = "a low-poly fox".into();
    } else {
        r.base.prompt = String::new();
        r.base.reference_images = vec![image_ref()];
    }
    r
}

type Setter = fn(&mut Model3dGenerationRequest);
type IsSet = fn(&Model3dGenerationRequest) -> bool;

/// Every canonical 3D param, as (name, set-it, is-it-still-set).
fn every_3d_param() -> Vec<(&'static str, Setter, IsSet)> {
    vec![
        ("texture", |r| r.texture = Some(true), |r| r.texture.is_some()),
        ("pbr", |r| r.pbr = Some(true), |r| r.pbr.is_some()),
        ("rig", |r| r.rig = Some(true), |r| r.rig.is_some()),
        ("target_polycount", |r| r.target_polycount = Some(5000), |r| r.target_polycount.is_some()),
        ("symmetry", |r| r.symmetry = Some("auto".into()), |r| r.symmetry.is_some()),
        ("topology", |r| r.topology = Some("quad".into()), |r| r.topology.is_some()),
        ("negative_prompt", |r| r.base.negative_prompt = Some("blurry".into()), |r| r.base.negative_prompt.is_some()),
    ]
}

#[test]
fn every_3d_param_is_refused_in_strict_and_dropped_in_lax_where_undeclared() {
    // fal/trellis declares output_formats and seed and nothing else.
    let schema = real("fal/trellis");
    for (param, set, is_set) in every_3d_param() {
        assert!(!schema.params.contains_key(param), "fixture assumption: trellis does not declare {param}");

        let mut r = valid_for(&schema, true);
        set(&mut r);
        let err = validate_model3d(&schema, r).unwrap_err();
        assert_eq!(err.code, "param_unsupported", "{param}");
        assert_eq!(err.param.as_deref(), Some(param));

        let mut r = valid_for(&schema, false);
        set(&mut r);
        let out = validate_model3d(&schema, r).unwrap_or_else(|e| panic!("{param} in lax: {e:?}"));
        assert!(out.dropped.iter().any(|d| d == param), "{param} must be reported dropped: {:?}", out.dropped);
        assert!(!is_set(&out.request), "{param} must be cleared from the request, not just reported");
    }
}

#[test]
fn every_declared_3d_param_passes_through_on_the_model_that_declares_it() {
    // mock/all-params-3d is the catalog's full-surface subject.
    let schema = real("mock/all-params-3d");
    for (param, set, is_set) in every_3d_param() {
        if param == "negative_prompt" {
            continue; // no 3D model in the catalog declares it
        }
        let mut r = valid_for(&schema, true);
        set(&mut r);
        let out = validate_model3d(&schema, r).unwrap_or_else(|e| panic!("{param}: {e:?}"));
        assert!(out.dropped.is_empty(), "{param}: {:?}", out.dropped);
        assert!(is_set(&out.request), "{param} must survive validation");
    }
}

#[test]
fn rodins_seed_ceiling_is_its_u16_maximum() {
    // Hyper3dRodinInput.seed is 0..=65535; fal answers more with a 422.
    let schema = real("fal/hyper3d-rodin");
    let mut r = valid_for(&schema, true);
    r.base.seed = Some(65_535);
    assert!(validate_model3d(&schema, r).is_ok());
    let mut r = valid_for(&schema, true);
    r.base.seed = Some(65_536);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "param_out_of_range");
    assert_eq!(err.param.as_deref(), Some("seed"));
}

#[test]
fn rodins_prompt_limit_is_1024_characters_not_bytes() {
    // Hyper3dRodinInput.prompt is `maxLength: 1024`, which JSON Schema counts
    // in characters. A byte count would refuse a non-ASCII prompt fal accepts.
    let schema = real("fal/hyper3d-rodin");
    let mut r = valid_for(&schema, true);
    r.base.prompt = "é".repeat(1024); // 2048 bytes, 1024 characters
    assert!(validate_model3d(&schema, r).is_ok(), "1024 characters is within the limit");
    let mut r = valid_for(&schema, true);
    r.base.prompt = "a".repeat(1025);
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "prompt_too_long");
}

#[test]
fn image_only_models_take_no_prompt() {
    // TrellisInput and Hunyuan3dV2Input define no prompt field. A prompt sent
    // there would have gone nowhere while the call answered 200.
    for id in ["fal/trellis", "fal/hunyuan3d-v2"] {
        let schema = real(id);
        assert_eq!(schema.prompt.max_length, Some(0), "{id}");

        let mut r = valid_for(&schema, true);
        r.base.prompt = "a fox".into();
        let err = validate_model3d(&schema, r).unwrap_err();
        assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_unsupported", Some("prompt")), "{id}");

        let mut r = valid_for(&schema, false);
        r.base.prompt = "a fox".into();
        let out = validate_model3d(&schema, r).unwrap();
        assert!(out.dropped.iter().any(|d| d == "prompt"), "{id}: {:?}", out.dropped);
        assert!(out.request.base.prompt.is_empty(), "{id}: a dropped prompt is cleared");
    }
}

#[test]
fn image_only_models_require_their_image() {
    for id in ["fal/trellis", "fal/hunyuan3d-v2"] {
        let schema = real(id);
        let mut r = valid_for(&schema, true);
        r.base.reference_images.clear();
        let err = validate_model3d(&schema, r).unwrap_err();
        assert_eq!(err.code, "ref_role_required", "{id}: {}", err.message);
    }
}

#[test]
fn text_to_3d_needs_a_prompt_even_where_the_prompt_is_optional() {
    // Rodin's prompt is optional because images make it so — but a request
    // with neither has nothing to generate from and fal would refuse it.
    let schema = real("fal/hyper3d-rodin");
    assert!(!schema.prompt.required, "fixture assumption");

    let mut r = valid_for(&schema, true);
    r.base.prompt = "  ".into();
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!((err.code.as_str(), err.param.as_deref()), ("prompt_required", Some("prompt")));

    let mut r = valid_for(&schema, true);
    r.base.prompt = String::new();
    r.base.reference_images = vec![image_ref()];
    assert!(validate_model3d(&schema, r).is_ok(), "image-to-3D needs no prompt");
}

#[test]
fn rodin_serves_the_derivable_set_from_one_job_but_not_two_native_formats() {
    let schema = real("fal/hyper3d-rodin");
    let ask = |formats: &[&str]| {
        let mut r = valid_for(&schema, true);
        r.output_formats = Some(formats.iter().map(|f| f.to_string()).collect());
        validate_model3d(&schema, r)
    };
    assert!(ask(&["glb", "obj", "stl", "ply"]).is_ok(), "one glb, three derived");
    assert!(ask(&["fbx"]).is_ok(), "one native fbx");
    assert!(ask(&["usdz"]).is_ok(), "one native usdz");
    // The budget follows what the converter can make. While fbx cannot be
    // derived, glb + fbx needs two Rodin jobs and a scalar field gives one;
    // once it can, one Rodin job (fbx) serves both.
    if is_derivable("fbx") {
        assert!(ask(&["glb", "fbx"]).is_ok(), "fbx from Rodin, glb derived from it");
    } else {
        assert_eq!(ask(&["glb", "fbx"]).unwrap_err().code, "param_too_many", "two vendor jobs");
        assert_eq!(ask(&["fbx", "obj"]).unwrap_err().code, "param_too_many", "obj cannot be derived from fbx");
    }
    let threemf = ask(&["3mf"]);
    if is_derivable("3mf") {
        assert!(threemf.is_ok(), "derived from the glb");
    } else {
        assert_eq!(threemf.unwrap_err().code, "param_enum_mismatch", "Rodin cannot emit 3mf and we cannot derive it");
    }
}

#[test]
fn glb_only_models_deliver_the_whole_derivable_set() {
    for id in ["fal/trellis", "fal/hunyuan3d-v2", "mock/mesh-3d"] {
        let schema = real(id);
        let mut r = valid_for(&schema, true);
        r.output_formats = Some(vec!["obj".into(), "stl".into(), "ply".into()]);
        assert!(validate_model3d(&schema, r).is_ok(), "{id}");

        // Every container the converter makes, fbx and usdz included.
        let mut r = valid_for(&schema, true);
        r.output_formats = Some(litegen::proxy::model3d_convert::derivable_formats());
        assert!(validate_model3d(&schema, r).is_ok(), "{id}: the whole derivable set");

        let mut r = valid_for(&schema, true);
        r.output_formats = Some(vec!["blend".into()]);
        let err = validate_model3d(&schema, r).unwrap_err();
        assert_eq!(err.code, "param_enum_mismatch", "{id}");
        assert!(err.message.contains("ply"), "{id}: the error lists what is deliverable: {}", err.message);
    }
}

#[test]
fn texture_and_pbr_go_only_where_the_vendor_has_a_field_for_them() {
    // Hunyuan3D maps `texture` onto `textured_mesh`; Rodin maps `pbr` onto
    // `material`. Neither has the other's knob.
    let hunyuan = real("fal/hunyuan3d-v2");
    let mut r = valid_for(&hunyuan, true);
    r.texture = Some(true);
    assert!(validate_model3d(&hunyuan, r).is_ok());
    let mut r = valid_for(&hunyuan, true);
    r.pbr = Some(true);
    assert_eq!(validate_model3d(&hunyuan, r).unwrap_err().param.as_deref(), Some("pbr"));

    let rodin = real("fal/hyper3d-rodin");
    let mut r = valid_for(&rodin, true);
    r.pbr = Some(false);
    assert!(validate_model3d(&rodin, r).is_ok());
    let mut r = valid_for(&rodin, true);
    r.texture = Some(true);
    assert_eq!(validate_model3d(&rodin, r).unwrap_err().param.as_deref(), Some("texture"));
}

#[test]
fn a_vendor_field_outside_the_allowlist_is_refused_in_strict_mode() {
    // `extra` is the escape hatch for endpoint fields the canonical vocabulary
    // does not cover — bounded by each model's extra_allowlist.
    let schema = real("fal/trellis");
    let mut r = valid_for(&schema, true);
    r.base.extra = Some(serde_json::json!({ "mesh_simplify": 0.95, "texture_size": 2048 }));
    assert!(validate_model3d(&schema, r).is_ok());
    let mut r = valid_for(&schema, true);
    r.base.extra = Some(serde_json::json!({ "geometry_file_format": "fbx" }));
    let err = validate_model3d(&schema, r).unwrap_err();
    assert_eq!(err.code, "extra_key_unsupported");
    assert_eq!(err.param.as_deref(), Some("extra.geometry_file_format"));
}
