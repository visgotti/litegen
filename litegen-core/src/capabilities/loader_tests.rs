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
