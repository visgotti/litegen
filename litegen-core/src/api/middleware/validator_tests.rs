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

    // ─── Endpoint ↔ media-type family ───────────────────────────────────────
    //
    // Each Validated* extractor serves one family and runs `require_media_type`
    // right after the schema lookup, before any param validation.

    use crate::capabilities::MediaType;
    use axum::http::StatusCode;

    fn family_registry() -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("families.yaml", r#"
models:
  - id: f/image
    provider: f
    media_type: image
    display_name: Image
    pricing: { base_cost_usd: 0.0 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
  - id: f/video
    provider: f
    media_type: video
    display_name: Video
    pricing: { base_cost_usd: 0.0 }
    capabilities: { text_to_video: true }
    prompt: { required: true }
  - id: f/mesh
    provider: f
    media_type: model3d
    display_name: Mesh
    pricing: { base_cost_usd: 0.0 }
    capabilities: { text_to_3d: true }
    prompt: { required: true }
"#)]).unwrap()
    }

    #[test]
    fn a_model_from_another_family_is_rejected_naming_the_endpoint_that_serves_it() {
        let r = family_registry();
        // (family the endpoint serves, model sent, endpoint the error must name)
        let cases = [
            (MediaType::Image, "f/video", "/v1/videos/generations"),
            (MediaType::Image, "f/mesh", "/v1/models3d/generations"),
            (MediaType::Video, "f/image", "/v1/images/generations"),
            (MediaType::Video, "f/mesh", "/v1/models3d/generations"),
            (MediaType::Model3d, "f/image", "/v1/images/generations"),
            (MediaType::Model3d, "f/video", "/v1/videos/generations"),
        ];
        for (served, model, endpoint) in cases {
            let Err(ValidationRejection(status, body)) = require_media_type(r.get(model).unwrap(), served) else {
                panic!("{model} on the {served:?} endpoint must be rejected");
            };
            assert_eq!(status, StatusCode::BAD_REQUEST, "{model} on {served:?}");
            assert_eq!(body["error"]["type"], "validation_error", "{model} on {served:?}");
            assert_eq!(body["error"]["code"], "model_media_type_mismatch", "{model} on {served:?}");
            assert_eq!(body["error"]["param"], "model", "{model} on {served:?}");
            assert_eq!(body["error"]["model"], model, "{model} on {served:?}");
            let msg = body["error"]["message"].as_str().unwrap();
            assert!(msg.contains(model), "{model} on {served:?}: {msg}");
            assert!(msg.contains(endpoint), "{model} on {served:?}: {msg}");
        }
    }

    #[test]
    fn a_model_from_the_served_family_passes() {
        let r = family_registry();
        for (served, model) in [
            (MediaType::Image, "f/image"),
            (MediaType::Video, "f/video"),
            (MediaType::Model3d, "f/mesh"),
        ] {
            assert!(require_media_type(r.get(model).unwrap(), served).is_ok(), "{model} on {served:?}");
        }
    }

    // ─── duration_seconds: omitted means the model's own default ─────────────

    fn video_registry() -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("v.yaml", r#"
models:
  - id: v/six
    provider: v
    media_type: video
    display_name: Six
    pricing: { base_cost_usd: 0.5 }
    capabilities: { text_to_video: true }
    prompt: { required: true, max_length: 100 }
    params:
      duration_seconds: { kind: float, min: 6.0, max: 10.0, default: 6.0 }
  - id: v/eight
    provider: v
    media_type: video
    display_name: Eight
    pricing: { base_cost_usd: 0.5 }
    capabilities: { text_to_video: true }
    prompt: { required: true, max_length: 100 }
    params:
      duration_seconds: { kind: int, min: 8, max: 8, default: 8 }
  - id: v/none
    provider: v
    media_type: video
    display_name: NoDuration
    pricing: { base_cost_usd: 0.5 }
    capabilities: { text_to_video: true }
    prompt: { required: true, max_length: 100 }
    params: {}
"#)]).unwrap()
    }

    /// A request body carrying no duration at all, as a caller would send it.
    fn vid_json(model: &str) -> VideoGenerationRequest {
        serde_json::from_value(serde_json::json!({ "model": model, "prompt": "a cat" }))
            .expect("video request without duration_seconds should deserialize")
    }

    /// A model whose minimum is above the legacy 5s fallback must still accept a
    /// request that omits duration: the omission means "use the model's default",
    /// not "5 seconds". Before this, every such request was rejected
    /// (fal/ltx-2.3 at 6s and bedrock nova-reel at 6s could not be called
    /// without passing duration explicitly).
    #[test]
    fn omitted_duration_is_accepted_by_a_model_whose_minimum_is_above_five() {
        let r = video_registry();
        for model in ["v/six", "v/eight"] {
            let out = validate_video(r.get(model).unwrap(), vid_json(model));
            assert!(out.is_ok(), "{model}: omitted duration was rejected: {:?}", out.err());
        }
    }

    /// Omitted resolves to the model's declared default; an explicit value wins.
    #[test]
    fn resolved_duration_prefers_the_request_then_the_models_default() {
        let r = video_registry();
        let six = r.get("v/six").unwrap();
        assert_eq!(resolve_duration_seconds(six, None), 6.0);
        assert_eq!(resolve_duration_seconds(six, Some(9.5)), 9.5);
        assert_eq!(resolve_duration_seconds(r.get("v/eight").unwrap(), None), 8.0);
        // No duration param at all: the legacy 5s fallback still applies.
        assert_eq!(resolve_duration_seconds(r.get("v/none").unwrap(), None), 5.0);
    }

    /// The range check still applies to a value the caller actually sent.
    #[test]
    fn an_explicit_duration_outside_the_range_is_still_rejected() {
        let r = video_registry();
        let mut req = vid_json("v/six");
        req.duration_seconds = Some(3.0);
        let err = validate_video(r.get("v/six").unwrap(), req).unwrap_err();
        assert_eq!(err.code, "param_out_of_range");
        assert_eq!(err.param.as_deref(), Some("duration_seconds"));
    }
}
