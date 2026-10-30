#[cfg(test)]
mod tests {
    use crate::capabilities::*;

    fn yaml(s: &str) -> Result<CapabilityRegistry, LoadError> {
        CapabilityRegistry::from_yaml_strs(&[("test.yaml", s)])
    }

    #[test]
    fn loads_single_model() {
        let r = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
"#).unwrap();
        assert_eq!(r.len(), 1);
        assert_eq!(r.get("x/m").unwrap().id, "x/m");
    }

    #[test]
    fn rejects_duplicate_id() {
        let err = CapabilityRegistry::from_yaml_strs(&[
            ("a.yaml", "models:\n  - id: x/m\n    provider: x\n    media_type: image\n    display_name: M\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
            ("b.yaml", "models:\n  - id: x/m\n    provider: x\n    media_type: image\n    display_name: M\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
        ]).unwrap_err();
        let s = err.to_string();
        assert!(s.contains("duplicate"));
        assert!(s.contains("x/m"));
    }

    #[test]
    fn rejects_provider_id_mismatch() {
        let err = yaml(r#"
models:
  - id: foo/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
"#).unwrap_err();
        assert!(err.to_string().contains("provider prefix"));
    }

    #[test]
    fn rejects_unknown_param_key() {
        let err = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    params:
      unknown_param:
        kind: int
"#).unwrap_err();
        assert!(err.to_string().contains("unknown_param"));
    }

    #[test]
    fn rejects_field_map_undeclared_role() {
        let err = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format:
        form: multipart
        field_map: { init: image, ghost: mask }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
"#).unwrap_err();
        assert!(err.to_string().contains("ghost"));
    }

    #[test]
    fn rejects_freeform_min_gt_max() {
        let err = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    params:
      size:
        kind: size
        mode: freeform
        min_width: 1000
        max_width: 100
        min_height: 100
        max_height: 1000
"#).unwrap_err();
        assert!(err.to_string().contains("min_width"));
    }

    // ─── output_formats ─────────────────────────────────────────────────────
    //
    // These checks are what keep a model from advertising a container its
    // provider cannot emit — the failure this param exists to close, and one
    // that is otherwise invisible until a caller is billed for a generation
    // whose promised format never arrives.

    fn model3d_yaml(params: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&format!(r#"
models:
  - id: x/m
    provider: x
    media_type: model3d
    display_name: M
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ text_to_3d: true }}
    prompt: {{ required: true }}
    params:
{params}
"#))
    }

    #[test]
    fn accepts_a_well_formed_output_formats_spec() {
        let r = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [glb, obj]\n        max_items: 2\n        default: [glb]\n",
        ).unwrap();
        let spec = r.get("x/m").unwrap().output_formats_spec().unwrap();
        assert_eq!(spec.enum_values, vec!["glb", "obj"]);
        assert_eq!(spec.max_items, Some(2));
    }

    #[test]
    fn rejects_output_formats_declared_with_the_wrong_kind() {
        // `output_formats_spec` reads a kind mismatch as "no spec at all", which
        // would quietly disable format validation for the whole model — so the
        // catalog must not load in the first place.
        let err = model3d_yaml(
            "      output_formats:\n        kind: string\n        enum_values: [glb]\n",
        ).unwrap_err();
        assert!(err.to_string().contains("string_array"), "{err}");
    }

    #[test]
    fn rejects_an_output_formats_spec_without_glb() {
        let err = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [obj, stl]\n",
        ).unwrap_err();
        assert!(err.to_string().contains("glb"), "{err}");
    }

    #[test]
    fn rejects_max_items_larger_than_the_declared_formats() {
        let err = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [glb]\n        max_items: 3\n",
        ).unwrap_err();
        assert!(err.to_string().contains("max_items"), "{err}");
    }

    #[test]
    fn rejects_zero_max_items() {
        let err = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [glb]\n        max_items: 0\n",
        ).unwrap_err();
        assert!(err.to_string().contains("max_items"), "{err}");
    }

    #[test]
    fn rejects_a_default_outside_the_declared_formats() {
        let err = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [glb]\n        default: [fbx]\n",
        ).unwrap_err();
        assert!(err.to_string().contains("default"), "{err}");
    }

    #[test]
    fn rejects_a_default_longer_than_max_items() {
        let err = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [glb, obj]\n        max_items: 1\n        default: [glb, obj]\n",
        ).unwrap_err();
        assert!(err.to_string().contains("default"), "{err}");
    }

    #[test]
    fn rejects_a_model_carrying_both_format_spellings() {
        let err = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [glb]\n      output_format:\n        kind: string\n        enum_values: [glb]\n",
        ).unwrap_err();
        assert!(err.to_string().contains("superseded"), "{err}");
    }

    #[test]
    fn the_superseded_scalar_spelling_still_loads_on_its_own() {
        // A deployment pointing at its own models dir must keep booting across
        // the release that introduces the plural.
        let r = model3d_yaml(
            "      output_format:\n        kind: string\n        enum_values: [glb, obj]\n        default: glb\n",
        ).unwrap();
        let spec = r.get("x/m").unwrap().output_formats_spec().unwrap();
        assert_eq!(spec.enum_values, vec!["glb", "obj"]);
        // A scalar vendor field can only ever produce one container per job.
        assert_eq!(spec.max_items, Some(1));
        assert_eq!(spec.default, vec!["glb"]);
    }

    #[test]
    fn rejects_a_canonical_param_declared_with_the_wrong_kind() {
        // It would load, `supports_pbr` would say yes, and the validator — which
        // reads `pbr` only as a Bool — would refuse it on every request.
        let err = model3d_yaml("      pbr: { kind: string, enum_values: [on, off] }\n").unwrap_err();
        let msg = err.to_string();
        assert!(msg.contains("'pbr'") && msg.contains("kind: bool"), "{msg}");
    }

    #[test]
    fn duration_seconds_may_be_a_float_or_an_int() {
        for kind in ["float", "int"] {
            let r = yaml(&format!(r#"
models:
  - id: x/v
    provider: x
    media_type: video
    display_name: V
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ text_to_video: true }}
    prompt: {{ required: true }}
    params:
      duration_seconds: {{ kind: {kind}, min: 1, max: 10 }}
"#));
            assert!(r.is_ok(), "{kind}: {:?}", r.err());
        }
    }

    #[test]
    fn rejects_a_docs_url_that_is_not_absolute_https() {
        for bad in ["http://fal.ai/models/x/api", "fal.ai/models/x/api", "/models/x/api", "https://"] {
            let err = yaml(&format!(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    docs_url: "{bad}"
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ text_to_image: true }}
    prompt: {{ required: true }}
"#)).unwrap_err();
            assert!(err.to_string().contains("docs_url"), "{bad}: {err}");
        }
    }

    #[test]
    fn an_https_docs_url_loads_and_is_kept() {
        let r = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    docs_url: https://fal.ai/models/fal-ai/trellis/api
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
"#).unwrap();
        assert_eq!(
            r.get("x/m").unwrap().docs_url.as_deref(),
            Some("https://fal.ai/models/fal-ai/trellis/api"),
        );
    }

    #[test]
    fn rejects_an_output_format_outside_the_3d_vocabulary() {
        // A typo here would advertise a container no observer of completion
        // could ever find, failing every generation that asked for it.
        let err = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [glb, gbl]\n",
        ).unwrap_err();
        assert!(err.to_string().contains("'gbl'"), "{err}");
    }

    #[test]
    fn the_3d_vocabulary_is_spelled_canonically() {
        // Validation folds case, but the catalog is what clients are served.
        let err = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [glb, OBJ]\n",
        ).unwrap_err();
        assert!(err.to_string().contains("'OBJ'"), "{err}");
    }

    #[test]
    fn rejects_topology_and_symmetry_outside_their_vocabularies() {
        let err = model3d_yaml("      topology: { kind: string, enum_values: [triangle, voxel] }\n").unwrap_err();
        assert!(err.to_string().contains("'voxel'"), "{err}");
        let err = model3d_yaml("      symmetry: { kind: string, enum_values: [off, mirror] }\n").unwrap_err();
        assert!(err.to_string().contains("'mirror'"), "{err}");
    }

    #[test]
    fn rejects_skeleton_and_joint_naming_outside_their_vocabularies() {
        // 3D doc §3B: Tripo's rig_type values with its biped named humanoid,
        // and the joint conventions a vendor or litegen can produce.
        let err = model3d_yaml("      skeleton: { kind: string, enum_values: [humanoid, dragon] }\n").unwrap_err();
        assert!(err.to_string().contains("'dragon'"), "{err}");
        let err = model3d_yaml("      joint_naming: { kind: string, enum_values: [vendor, blender] }\n").unwrap_err();
        assert!(err.to_string().contains("'blender'"), "{err}");
        assert!(model3d_yaml(
            "      skeleton: { kind: string, enum_values: [humanoid, quadruped, hexapod, octopod, avian, serpentine, aquatic, other] }\n      joint_naming: { kind: string, enum_values: [vendor, vrm, mixamo, unity, unreal] }\n      height_m: { kind: float, min: 0.1, max: 10.0 }\n",
        ).is_ok());
    }

    #[test]
    fn height_m_is_a_float() {
        let err = model3d_yaml("      height_m: { kind: int, min: 1, max: 3 }\n").unwrap_err();
        assert!(err.to_string().contains("height_m"), "{err}");
    }

    #[test]
    fn rejects_a_required_prompt_that_may_not_be_sent() {
        let err = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: model3d
    display_name: M
    pricing: { base_cost_usd: 0.01 }
    capabilities: { image_to_3d: true }
    prompt: { required: true, max_length: 0 }
"#).unwrap_err();
        assert!(err.to_string().contains("max_length is 0"), "{err}");
    }

    #[test]
    fn derivable_values_cannot_be_set_from_yaml() {
        // It is computed by the server from what the vendor emits; a catalog
        // that could set it could promise conversions litegen cannot perform.
        let err = model3d_yaml(
            "      output_formats:\n        kind: string_array\n        enum_values: [glb]\n        derivable_values: [fbx]\n",
        ).unwrap_err();
        assert!(err.to_string().contains("derivable_values"), "{err}");
    }

    #[test]
    fn for_provider_groups_correctly() {
        let r = CapabilityRegistry::from_yaml_strs(&[
            ("a.yaml", "models:\n  - id: x/a\n    provider: x\n    media_type: image\n    display_name: A\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n  - id: x/b\n    provider: x\n    media_type: image\n    display_name: B\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
            ("c.yaml", "models:\n  - id: y/c\n    provider: y\n    media_type: image\n    display_name: C\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
        ]).unwrap();
        assert_eq!(r.for_provider("x").count(), 2);
        assert_eq!(r.for_provider("y").count(), 1);
        assert_eq!(r.for_provider("z").count(), 0);
    }

    // ─── task (3D doc §2 rule 1) ────────────────────────────────────────────

    fn model3d(task_line: &str, caps: &str, params: &str) -> String {
        format!(r#"
models:
  - id: x/op
    provider: x
    media_type: model3d
    display_name: Op
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {caps}
    prompt: {{ required: false }}
{task_line}
{params}
"#)
    }

    #[test]
    fn task_defaults_to_generate() {
        let r = yaml(&model3d("", "{ image_to_3d: true }", "")).unwrap();
        assert_eq!(r.get("x/op").unwrap().task, ModelTask::Generate);
    }

    #[test]
    fn loads_a_3d_operation_task() {
        let r = yaml(&model3d("    task: rig", "{}", RIG_SOURCE)).unwrap();
        assert_eq!(r.get("x/op").unwrap().task, ModelTask::Rig);
    }

    #[test]
    fn rejects_a_task_from_another_modality() {
        let err = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    task: extend
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
"#).unwrap_err().to_string();
        assert!(err.contains("task 'extend'") && err.contains("image"), "{err}");
    }

    #[test]
    fn rejects_to_3d_flags_on_a_non_generate_task() {
        // The flags say how a GENERATION is driven; an operation takes a mesh.
        let err = yaml(&model3d("    task: rig", "{ image_to_3d: true }", RIG_SOURCE)).unwrap_err().to_string();
        assert!(err.contains("image_to_3d") && err.contains("generate"), "{err}");
    }

    #[test]
    fn rejects_an_unknown_task() {
        let err = yaml(&model3d("    task: teleport", "{}", RIG_SOURCE)).unwrap_err().to_string();
        assert!(err.contains("teleport"), "{err}");
    }

    // ─── source_model (3D doc §3A) ──────────────────────────────────────────

    const RIG_SOURCE: &str = "    params:\n      source_model: { kind: source_model, formats: [glb, fbx], max_bytes: 104857600, max_faces: 300000, requires_skin: false, requires_texture: true }";

    #[test]
    fn a_3d_operation_declares_what_source_it_takes() {
        let r = yaml(&model3d("    task: rig", "{}", RIG_SOURCE)).unwrap();
        match r.get("x/op").unwrap().params.get("source_model") {
            Some(ParamSpec::SourceModel(s)) => {
                assert_eq!(s.formats, vec!["glb".to_string(), "fbx".to_string()]);
                assert_eq!(s.max_bytes, Some(104_857_600));
                assert_eq!(s.max_faces, Some(300_000));
                assert!(!s.requires_skin && s.requires_texture);
            }
            other => panic!("source_model is {other:?}"),
        }
    }

    #[test]
    fn a_3d_operation_without_a_source_model_is_refused() {
        let err = yaml(&model3d("    task: animate", "{}", "")).unwrap_err().to_string();
        assert!(err.contains("source_model") && err.contains("animate"), "{err}");
    }

    #[test]
    fn source_model_formats_come_from_the_mesh_vocabulary() {
        let bad = "    params:\n      source_model: { kind: source_model, formats: [blend] }";
        let err = yaml(&model3d("    task: rig", "{}", bad)).unwrap_err().to_string();
        assert!(err.contains("blend"), "{err}");
        let empty = "    params:\n      source_model: { kind: source_model, formats: [] }";
        let err = yaml(&model3d("    task: rig", "{}", empty)).unwrap_err().to_string();
        assert!(err.contains("formats"), "{err}");
    }

    // ─── native_orientation (3D doc §2 rule 3) ──────────────────────────────

    fn oriented(line: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&model3d(line, "{ image_to_3d: true }", ""))
    }

    #[test]
    fn a_3d_model_declares_its_proven_orientation() {
        let r = oriented("    native_orientation: { up: \"+y\", front: \"+x\", evidence: \"a live run's sample\" }").unwrap();
        let o = r.get("x/op").unwrap().native_orientation.clone().unwrap();
        assert_eq!((o.up.as_str(), o.front.as_str(), o.evidence.as_str()), ("+y", "+x", "a live run's sample"));
        assert!(oriented("").unwrap().get("x/op").unwrap().native_orientation.is_none(), "unproven is the default");
    }

    #[test]
    fn native_orientation_axes_are_signed_axes_at_right_angles() {
        for (line, needle) in [
            ("    native_orientation: { up: \"y\", front: \"+z\", evidence: \"e\" }", "'y'"),
            ("    native_orientation: { up: \"+y\", front: \"forward\", evidence: \"e\" }", "'forward'"),
            ("    native_orientation: { up: \"+y\", front: \"-y\", evidence: \"e\" }", "right angles"),
        ] {
            let err = oriented(line).unwrap_err().to_string();
            assert!(err.contains(needle), "{line}: {err}");
        }
    }

    #[test]
    fn native_orientation_names_its_evidence() {
        let err = oriented("    native_orientation: { up: \"+y\", front: \"+z\", evidence: \"  \" }").unwrap_err().to_string();
        assert!(err.contains("evidence"), "{err}");
    }

    #[test]
    fn native_orientation_is_a_3d_field() {
        let err = yaml(r#"
models:
  - id: x/m
    provider: x
    media_type: image
    display_name: M
    native_orientation: { up: "+y", front: "+z", evidence: "e" }
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
"#).unwrap_err().to_string();
        assert!(err.contains("native_orientation") && err.contains("model3d"), "{err}");
    }

    // ─── animations (3D doc §3C) ────────────────────────────────────────────

    const ANIMATIONS: &str = "      source_model: { kind: source_model, formats: [glb], requires_skin: true }\n      animations:\n        kind: animation_list\n        max_items: 5\n        presets:\n          - { id: \"preset:walk\", canonical: walk, loopable: true }\n          - { id: \"preset:slash\", canonical: attack_melee }\n          - { id: \"preset:dive\" }\n        text_to_motion: { min_duration_s: 4, max_duration_s: 10 }\n        in_place: native\n";

    fn animate(params: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&model3d("    task: animate", "{}", &format!("    params:\n{params}")))
    }

    #[test]
    fn an_animate_model_declares_its_presets_and_motion_limits() {
        let r = animate(ANIMATIONS).unwrap();
        match r.get("x/op").unwrap().params.get("animations") {
            Some(ParamSpec::AnimationList(a)) => {
                assert_eq!(a.max_items, 5);
                assert_eq!(a.presets.len(), 3);
                assert_eq!(a.presets[0].canonical.as_deref(), Some("walk"));
                assert!(a.presets[0].loopable);
                assert_eq!(a.presets[2].canonical, None, "a vendor id with no canonical twin");
                let t = a.text_to_motion.as_ref().unwrap();
                assert_eq!((t.min_duration_s, t.max_duration_s), (4.0, 10.0));
                assert_eq!(a.in_place, InPlaceMode::Native);
            }
            other => panic!("animations is {other:?}"),
        }
    }

    #[test]
    fn a_preset_tagged_outside_the_canonical_vocabulary_is_refused() {
        let bad = ANIMATIONS.replace("canonical: attack_melee", "canonical: moonwalk");
        let err = animate(&bad).unwrap_err().to_string();
        assert!(err.contains("moonwalk"), "{err}");
    }

    #[test]
    fn preset_ids_are_unique_and_limits_are_sane() {
        let dup = ANIMATIONS.replace("preset:dive", "preset:walk");
        assert!(animate(&dup).unwrap_err().to_string().contains("preset:walk"));
        let none = ANIMATIONS.replace("max_items: 5", "max_items: 0");
        assert!(animate(&none).unwrap_err().to_string().contains("max_items"));
        let backwards = ANIMATIONS.replace("min_duration_s: 4, max_duration_s: 10", "min_duration_s: 10, max_duration_s: 4");
        assert!(animate(&backwards).unwrap_err().to_string().contains("text_to_motion"));
    }

    #[test]
    fn an_operation_may_keep_its_sources_orientation() {
        let line = format!("    task: animate\n    native_orientation: {{ up: source, front: source, evidence: \"returns the source mesh\" }}");
        let r = yaml(&model3d(&line, "{}", &format!("    params:\n{ANIMATIONS}"))).unwrap();
        let o = r.get("x/op").unwrap().native_orientation.clone().unwrap();
        assert_eq!((o.up.as_str(), o.front.as_str()), ("source", "source"));
        // Only an operation has a source to keep, and both axes go together.
        let err = oriented("    native_orientation: { up: source, front: source, evidence: \"e\" }").unwrap_err().to_string();
        assert!(err.contains("source"), "{err}");
        let half = format!("    task: animate\n    native_orientation: {{ up: \"+y\", front: source, evidence: \"e\" }}");
        assert!(yaml(&model3d(&half, "{}", &format!("    params:\n{ANIMATIONS}"))).is_err());
    }

    // ─── partial image edits (3D doc §4) ────────────────────────────────────

    fn editor(roles: &str, extra: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&format!(r#"
models:
  - id: x/edit
    provider: x
    media_type: image
    display_name: Edit
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ text_to_image: false, image_to_image: true, inpainting: true }}
    prompt: {{ required: true }}
{extra}
    ref_inputs:
      max_total: 2
      default_role: init
      provider_format: {{ form: base64 }}
      roles:
        init: {{ required: true, min_count: 1, max_count: 1 }}
{roles}
"#))
    }

    #[test]
    fn a_mask_role_names_its_vendors_convention() {
        let r = editor("        mask: { required: false, min_count: 0, max_count: 1, convention: black_edits }", "").unwrap();
        let m = r.get("x/edit").unwrap();
        let role = &m.ref_inputs.as_ref().unwrap().roles["mask"];
        assert_eq!(role.convention, Some(MaskConvention::BlackEdits));
        let err = editor("        mask: { required: false, min_count: 0, max_count: 1 }", "").unwrap_err().to_string();
        assert!(err.contains("convention"), "{err}");
    }

    #[test]
    fn native_grow_and_a_convention_belong_to_the_mask_role_only() {
        let err = editor("        mask: { required: false, min_count: 0, max_count: 1, convention: white_edits }\n        style: { required: false, min_count: 0, max_count: 1, native_grow: true }", "").unwrap_err().to_string();
        assert!(err.contains("native_grow") || err.contains("style"), "{err}");
    }

    #[test]
    fn a_region_model_declares_how_it_takes_the_region() {
        let params = "    params:\n      region: { kind: region, input: box, min_window_px: 256, max_aspect: 6.0 }\n    preserves_unmasked: exact";
        let r = editor("        mask: { required: false, min_count: 0, max_count: 1, convention: black_edits }", params).unwrap();
        let m = r.get("x/edit").unwrap();
        match m.params.get("region") {
            Some(ParamSpec::Region(s)) => assert_eq!((s.input, s.min_window_px, s.max_aspect), (RegionInput::Box, Some(256), Some(6.0))),
            other => panic!("{other:?}"),
        }
        assert_eq!(m.preserves_unmasked, Some(PreservesUnmasked::Exact));
    }

    #[test]
    fn preserves_unmasked_is_for_models_that_edit_part_of_an_image() {
        let err = yaml(r#"
models:
  - id: x/gen
    provider: x
    media_type: image
    display_name: Gen
    preserves_unmasked: exact
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
"#).unwrap_err().to_string();
        assert!(err.contains("preserves_unmasked"), "{err}");
    }

    // ─── Views (3D doc §3E) ──────────────────────────────────────────────────

    fn multiview_3d(roles: &str, view_order: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&format!(r#"
models:
  - id: x/mv
    provider: x
    media_type: model3d
    display_name: MV
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ multiview_to_3d: true }}
    prompt: {{ required: false, max_length: 0 }}
    ref_inputs:
      max_total: 4
      provider_format: {{ form: url }}
{view_order}
      roles:
{roles}
"#))
    }

    const FRONT_LEFT: &str = "        view-front: { required: true, min_count: 1, max_count: 1 }\n        view-left: { required: false, min_count: 0, max_count: 1 }";

    #[test]
    fn view_roles_come_from_the_one_view_vocabulary() {
        assert!(multiview_3d(FRONT_LEFT, "").is_ok());
        let err = multiview_3d("        view-front: { required: true, min_count: 1, max_count: 1 }\n        view-sideways: { required: false, min_count: 0, max_count: 1 }", "")
            .unwrap_err()
            .to_string();
        assert!(err.contains("view-sideways"), "{err}");
    }

    #[test]
    fn a_view_order_names_only_views_the_model_takes_once_each() {
        let r = multiview_3d(FRONT_LEFT, "      view_order: [front, left]").unwrap();
        assert_eq!(r.get("x/mv").unwrap().ref_inputs.as_ref().unwrap().view_order, ["front", "left"]);
        for order in ["      view_order: [front, back]", "      view_order: [front, left, front]", "      view_order: [front, sideways]"] {
            let err = multiview_3d(FRONT_LEFT, order).unwrap_err().to_string();
            assert!(err.contains("view_order"), "{order}: {err}");
        }
    }

    fn multiview_image(params: &str, roles: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&format!(r#"
models:
  - id: x/views
    provider: x
    media_type: image
    task: multiview
    display_name: Views
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ image_to_image: true }}
    prompt: {{ required: false, max_length: 0 }}
    params:
{params}
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: {{ form: base64 }}
      roles:
{roles}
"#))
    }

    const INIT: &str = "        init: { required: true, min_count: 1, max_count: 1 }";

    #[test]
    fn a_multiview_image_model_names_the_views_it_makes_and_takes_an_init_image() {
        let views = "      views: { kind: string_array, enum_values: [front, back], default: [front, back] }";
        assert!(multiview_image(views, INIT).is_ok());
        let err = multiview_image("      views: { kind: string_array, enum_values: [front, upside-down] }", INIT).unwrap_err().to_string();
        assert!(err.contains("upside-down"), "{err}");
        let err = multiview_image("      views: { kind: string, enum_values: [front] }", INIT).unwrap_err().to_string();
        assert!(err.contains("string_array"), "{err}");
        let err = multiview_image("      seed: { kind: seed, min: 0, max: 10 }", INIT).unwrap_err().to_string();
        assert!(err.contains("multiview"), "no views: {err}");
        let err = multiview_image(views, "        style: { required: true, min_count: 1, max_count: 1 }").unwrap_err().to_string();
        assert!(err.contains("multiview"), "no init role: {err}");
    }

    // ─── Keyframes (3D doc §5) ───────────────────────────────────────────────

    fn keyframe_video(media_type: &str, roles: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&format!(r#"
models:
  - id: x/kf
    provider: x
    media_type: {media_type}
    display_name: KF
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ image_to_video: true }}
    prompt: {{ required: true }}
    ref_inputs:
      max_total: 8
      provider_format: {{ form: url }}
      roles:
{roles}
"#))
    }

    #[test]
    fn a_keyframe_role_says_how_its_times_are_placed() {
        let r = keyframe_video("video", "        keyframe: { required: false, min_count: 2, max_count: 8, timing: any, time_step_s: 1.0 }").unwrap();
        let k = &r.get("x/kf").unwrap().ref_inputs.as_ref().unwrap().roles["keyframe"];
        assert_eq!((k.timing, k.time_step_s), (Some(KeyframeTiming::Any), Some(1.0)));
        for (media, roles, needle) in [
            ("video", "        keyframe: { required: false, min_count: 0, max_count: 8 }", "timing"),
            ("video", "        first_frame: { required: false, min_count: 0, max_count: 1, timing: any }", "timing"),
            ("video", "        keyframe: { required: false, min_count: 0, max_count: 8, timing: even, time_step_s: 0 }", "time_step_s"),
            ("image", "        keyframe: { required: false, min_count: 0, max_count: 8, timing: any }", "keyframe"),
        ] {
            let err = keyframe_video(media, roles).unwrap_err().to_string();
            assert!(err.contains(needle), "{roles}: {err}");
        }
    }

    // ─── Pricing rules (3D doc §3G, L14) ──────────────────────────────────────

    fn priced(media: &str, params: &str, rules: &str) -> Result<CapabilityRegistry, LoadError> {
        let caps = match media { "video" => "text_to_video: true", "model3d" => "text_to_3d: true", _ => "text_to_image: true" };
        yaml(&format!(r#"
models:
  - id: x/p
    provider: x
    media_type: {media}
    display_name: P
    pricing:
      base_cost_usd: 0.5
      variable_pricing:
{rules}
    capabilities: {{ {caps} }}
    prompt: {{ required: true }}
    params:
{params}
    extra_allowlist: [addons]
"#))
    }

    #[test]
    fn a_pricing_rule_names_one_effect_over_facts_the_request_has() {
        let video = "      duration_seconds: { kind: float, min: 1, max: 10, default: 5 }\n      resolution: { kind: string, enum_values: [720p, 1080p] }";
        assert!(priced("video", video, "        - { per_second_usd: 0.1 }\n        - { when: { resolution: 1080p, extra.addons: x }, multiply: 2 }").is_ok());
        for (media, rules, needle) in [
            ("video", "        - { per_second_usd: 0.1, multiply: 2 }", "one effect"),
            ("video", "        - { when: { resolution: 1080p } }", "one effect"),
            ("video", "        - { per_item_usd: 0.1 }", "per_item"),
            ("video", "        - { when: { quality: hd }, multiply: 2 }", "quality"),
            ("video", "        - { when: { extra.tier: x }, multiply: 2 }", "extra.tier"),
            ("image", "        - { per_second_usd: 0.1 }", "per_second_usd"),
            ("video", "        - { per_megapixel_usd: 0.1 }", "per_megapixel_usd"),
            ("video", "        - { usd: -1 }", "negative"),
        ] {
            let params = if media == "video" { video } else { "      {}" };
            let err = priced(media, params, rules).unwrap_err().to_string();
            assert!(err.contains(needle), "{rules}: {err}");
        }
    }

    // ─── Lifecycle (L14) ──────────────────────────────────────────────────────

    fn dated(fields: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&format!(r#"
models:
  - id: x/d
    provider: x
    media_type: image
    display_name: D
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ text_to_image: true }}
    prompt: {{ required: true }}
{fields}
"#))
    }

    #[test]
    fn lifecycle_dates_are_calendar_days_and_a_model_is_not_its_own_replacement() {
        assert!(dated("    deprecated_at: 2026-10-06\n    retires_at: 2027-03-15\n    replaced_by: x/e").is_ok());
        for (fields, needle) in [
            ("    retires_at: 15 March 2027", "retires_at"),
            ("    deprecated_at: 2026-13-01", "deprecated_at"),
            ("    replaced_by: x/d", "replaced_by"),
            ("    deprecated_at: 2027-01-01\n    retires_at: 2026-01-01", "retires_at"),
        ] {
            let err = dated(fields).unwrap_err().to_string();
            assert!(err.contains(needle), "{fields}: {err}");
        }
    }

    // ─── Segmentation (PixelGenie L7) ─────────────────────────────────────────

    fn segment_image(task: &str, caps: &str, params: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&format!(r#"
models:
  - id: x/seg
    provider: x
    media_type: image
{task}
    display_name: Seg
    pricing: {{ base_cost_usd: 0.005 }}
    capabilities: {caps}
    prompt: {{ required: false, max_length: 0 }}
    params:
{params}
"#))
    }

    const PROMPTS: &str = "      prompts: { kind: segmentation_prompts, types: [point, box, text], max_items: 16, max_masks: 32 }";

    #[test]
    fn a_segmentation_model_says_which_prompts_it_takes_and_makes_nothing_else() {
        let r = segment_image("    task: segment", "{}", PROMPTS).unwrap();
        let m = r.get("x/seg").unwrap();
        assert_eq!(m.task, ModelTask::Segment);
        match m.params.get("prompts") {
            Some(ParamSpec::SegmentationPrompts(p)) => assert_eq!(
                (p.types.clone(), p.max_items, p.max_masks),
                (vec![SegmentationPromptType::Point, SegmentationPromptType::Box, SegmentationPromptType::Text], 16, 32)
            ),
            other => panic!("{other:?}"),
        }
        for (task, caps, params, needle) in [
            ("    task: segment", "{}", "      seed: { kind: seed, min: 0, max: 10 }", "prompts"),
            ("", "{ text_to_image: true }", PROMPTS, "segment"),
            ("    task: segment", "{ text_to_image: true }", PROMPTS, "text_to_image"),
            ("    task: segment", "{}", "      prompts: { kind: segmentation_prompts, types: [], max_items: 16, max_masks: 32 }", "types"),
            ("    task: segment", "{}", "      prompts: { kind: segmentation_prompts, types: [point], max_items: 0, max_masks: 32 }", "max_items"),
            ("    task: segment", "{}", "      prompts: { kind: segmentation_prompts, types: [point], max_items: 4, max_masks: 0 }", "max_masks"),
            ("    task: segment", "{}", "      prompts: { kind: segmentation_prompts, types: [point, lasso], max_items: 4, max_masks: 1 }", "lasso"),
        ] {
            let err = segment_image(task, caps, params).unwrap_err().to_string();
            assert!(err.contains(needle), "{task} {caps} {params}: {err}");
        }
    }

    // ─── Background removal (PixelGenie L8) ───────────────────────────────────

    fn matting(caps: &str, prompt: &str, params: &str, init: &str) -> Result<CapabilityRegistry, LoadError> {
        yaml(&format!(r#"
models:
  - id: x/rmbg
    provider: x
    media_type: image
    display_name: RMBG
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {caps}
    prompt: {prompt}
    params:
{params}
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: {{ form: base64 }}
      roles:
        init: {init}
"#))
    }

    #[test]
    fn a_background_removal_model_takes_one_init_image_and_nothing_else() {
        const CAPS: &str = "{ background_removal: true }";
        const NO_PROMPT: &str = "{ required: false, max_length: 0 }";
        const INIT: &str = "{ required: true, min_count: 1, max_count: 1 }";
        const PNG: &str = "      output_format: { kind: string, enum_values: [png, webp], default: png }";
        let r = matting(CAPS, NO_PROMPT, PNG, INIT).unwrap();
        assert!(r.get("x/rmbg").unwrap().capabilities.background_removal);
        for (caps, prompt, params, init, needle) in [
            ("{ background_removal: true, image_to_image: true }", NO_PROMPT, PNG, INIT, "image_to_image"),
            (CAPS, "{ required: false, max_length: 100 }", PNG, INIT, "prompt"),
            (CAPS, NO_PROMPT, PNG, "{ required: false, min_count: 0, max_count: 1 }", "init"),
            (CAPS, NO_PROMPT, "      output_format: { kind: string, enum_values: [png, jpeg] }", INIT, "jpeg"),
        ] {
            let err = matting(caps, prompt, params, init).unwrap_err().to_string();
            assert!(err.contains(needle) && err.contains("background_removal"), "{caps} {prompt} {params} {init}: {err}");
        }
    }

    // ─── Texture operations (3D doc §3F) ──────────────────────────────────────

    #[test]
    fn a_texture_resolution_lists_the_sizes_the_model_paints_at() {
        let row = |spec: &str| yaml(&model3d("    task: texture", "{}", &format!("{RIG_SOURCE}\n      texture_resolution: {spec}")));
        assert!(row("{ kind: int, values: [1024, 2048] }").is_ok());
        let err = row("{ kind: int, min: 256, max: 4096 }").unwrap_err().to_string();
        assert!(err.contains("texture_resolution") && err.contains("values"), "{err}");
    }

    // ─── Expand (3D doc §4, P2) ───────────────────────────────────────────────

    #[test]
    fn an_expand_model_takes_the_init_image_it_grows() {
        let row = |roles: &str| yaml(&format!(r#"
models:
  - id: x/expand
    provider: x
    media_type: image
    display_name: Expand
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ image_to_image: true }}
    prompt: {{ required: false }}
    params:
      expand: {{ kind: expand, max_px: 100 }}
    ref_inputs:
      max_total: 1
      provider_format: {{ form: base64 }}
      roles:
{roles}
"#));
        assert!(row("        init: { required: true, min_count: 1, max_count: 1 }").is_ok());
        let err = row("        style: { required: false, min_count: 0, max_count: 1 }").unwrap_err().to_string();
        assert!(err.contains("expand") && err.contains("init"), "{err}");
    }
}
