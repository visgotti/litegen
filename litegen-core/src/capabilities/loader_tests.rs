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
    fn for_provider_groups_correctly() {
        let r = CapabilityRegistry::from_yaml_strs(&[
            ("a.yaml", "models:\n  - id: x/a\n    provider: x\n    media_type: image\n    display_name: A\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n  - id: x/b\n    provider: x\n    media_type: image\n    display_name: B\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
            ("c.yaml", "models:\n  - id: y/c\n    provider: y\n    media_type: image\n    display_name: C\n    pricing: { base_cost_usd: 0.01 }\n    capabilities: { text_to_image: true }\n    prompt: { required: true }\n"),
        ]).unwrap();
        assert_eq!(r.for_provider("x").count(), 2);
        assert_eq!(r.for_provider("y").count(), 1);
        assert_eq!(r.for_provider("z").count(), 0);
    }
}
