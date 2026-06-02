#[cfg(test)]
mod tests {
    use crate::api::middleware::validator::*;
    use crate::capabilities::*;
    use crate::types::*;

    fn registry() -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("t.yaml", r#"
models:
  - id: t/strict
    provider: t
    media_type: image
    display_name: T
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 100 }
    params:
      seed:
        kind: seed
        min: 0
        max: 100
      guidance_scale:
        kind: float
        min: 0.0
        max: 10.0
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9"]
      size:
        kind: size
        mode: enum
        values:
          - [512, 512]
          - [1024, 1024]
    extra_allowlist: [output_format]
    ref_inputs:
      max_total: 2
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
        mask: { required: false, min_count: 0, max_count: 1 }
"#)]).unwrap()
    }

    fn img(prompt: &str) -> ImageGenerationRequest {
        ImageGenerationRequest {
            base: BaseGenerationRequest {
                prompt: prompt.into(),
                model: "t/strict".into(),
                n: 1,
                negative_prompt: None,
                seed: None,
                reference_images: vec![],
                strict: true,
                extra: None,
                metadata: None,
            },
            size: None, aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None,
            response_format: "url".into(),
        }
    }

    #[test]
    fn empty_request_passes() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let out = validate_image(m, img("hello")).unwrap();
        assert!(out.dropped.is_empty());
    }

    #[test]
    fn unsupported_param_strict_rejects() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.steps = Some(20);
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_unsupported");
        assert_eq!(err.param.as_deref(), Some("steps"));
    }

    #[test]
    fn unsupported_param_lax_drops() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.strict = false;
        req.steps = Some(20);
        let out = validate_image(m, req).unwrap();
        assert_eq!(out.dropped, vec!["steps".to_string()]);
        assert!(out.request.steps.is_none());
    }

    #[test]
    fn seed_out_of_range() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.seed = Some(1000);
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_out_of_range");
        assert_eq!(err.param.as_deref(), Some("seed"));
    }

    #[test]
    fn float_out_of_range() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.guidance_scale = Some(20.0);
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_out_of_range");
        assert_eq!(err.param.as_deref(), Some("guidance_scale"));
    }

    #[test]
    fn aspect_ratio_not_allowed() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.aspect_ratio = Some("3:2".into());
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_enum_mismatch");
        assert_eq!(err.param.as_deref(), Some("aspect_ratio"));
    }

    #[test]
    fn size_enum_must_match() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.size = Some("768x768".into());
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "param_enum_mismatch");
        assert_eq!(err.param.as_deref(), Some("size"));
    }

    #[test]
    fn size_enum_passes_when_matches() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.size = Some("1024x1024".into());
        assert!(validate_image(m, req).is_ok());
    }

    #[test]
    fn prompt_too_long() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let req = img(&"x".repeat(200));
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "prompt_too_long");
    }

    #[test]
    fn prompt_required_when_empty() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let req = img("");
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "prompt_required");
    }

    #[test]
    fn ref_images_total_exceeded() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.reference_images = vec![
            ReferenceImage { kind: RefImageKind::Url, value: "u1".into(), role: Some("init".into()) },
            ReferenceImage { kind: RefImageKind::Url, value: "u2".into(), role: Some("mask".into()) },
            ReferenceImage { kind: RefImageKind::Url, value: "u3".into(), role: None },
        ];
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "ref_total_exceeded");
    }

    #[test]
    fn ref_role_count_exceeded() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.reference_images = vec![
            ReferenceImage { kind: RefImageKind::Url, value: "u1".into(), role: Some("init".into()) },
            ReferenceImage { kind: RefImageKind::Url, value: "u2".into(), role: Some("init".into()) },
        ];
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "ref_role_count_out_of_range");
    }

    #[test]
    fn ref_unknown_role_strict_rejects() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.reference_images = vec![
            ReferenceImage { kind: RefImageKind::Url, value: "u".into(), role: Some("ghost".into()) },
        ];
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "ref_role_unknown");
    }

    #[test]
    fn ref_unknown_role_lax_drops() {
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.strict = false;
        req.base.reference_images = vec![
            ReferenceImage { kind: RefImageKind::Url, value: "u".into(), role: Some("ghost".into()) },
        ];
        let out = validate_image(m, req).unwrap();
        assert!(out.request.base.reference_images.is_empty());
        assert!(out.dropped.contains(&"reference_images[ghost]".to_string()));
    }

    #[test]
    fn extra_allowlist_strict() {
        use serde_json::json;
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.extra = Some(json!({"output_format": "png", "ghost": 1}));
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "extra_key_unsupported");
    }

    #[test]
    fn extra_lax_passes_through() {
        use serde_json::json;
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.strict = false;
        req.base.extra = Some(json!({"output_format": "png", "ghost": 1}));
        let out = validate_image(m, req).unwrap();
        assert!(out.request.base.extra.is_some());
    }
}
