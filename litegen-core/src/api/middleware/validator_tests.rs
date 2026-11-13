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
      resolution: { kind: string, enum_values: ["1K", "2K"], default: "1K" }
    extra_allowlist: [output_format]
    ref_inputs:
      max_total: 2
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
        mask: { required: false, min_count: 0, max_count: 1, convention: white_edits }
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
            size: None, resolution: None, aspect_ratio: None, quality: None, style: None,
            steps: None, guidance_scale: None, strength: None,
            response_format: "url".into(),
            region: None, mask_grow_px: None, views: None,
            output_format: None, background: None,
            expand: None,
            layout: None, mask_prompt: None,
            preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
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
            ReferenceImage { kind: RefImageKind::Url, value: "u1".into(), role: Some("init".into()), time_s: None },
            ReferenceImage { kind: RefImageKind::Url, value: "u2".into(), role: Some("mask".into()), time_s: None },
            ReferenceImage { kind: RefImageKind::Url, value: "u3".into(), role: None, time_s: None },
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
            ReferenceImage { kind: RefImageKind::Url, value: "u1".into(), role: Some("init".into()), time_s: None },
            ReferenceImage { kind: RefImageKind::Url, value: "u2".into(), role: Some("init".into()), time_s: None },
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
            ReferenceImage { kind: RefImageKind::Url, value: "u".into(), role: Some("ghost".into()), time_s: None },
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
            ReferenceImage { kind: RefImageKind::Url, value: "u".into(), role: Some("ghost".into()), time_s: None },
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

    /// An `extra` key that is one of the model's own params (a caller from
    /// before it became one, as `background` and `output_format` did) is
    /// told where it goes now.
    #[test]
    fn an_extra_key_that_is_a_param_says_to_send_it_as_one() {
        use serde_json::json;
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.extra = Some(json!({ "guidance_scale": 7.5 }));
        let err = validate_image(m, req).unwrap_err();
        assert_eq!(err.code, "extra_key_unsupported");
        assert!(err.message.contains("send it as `guidance_scale`"), "{}", err.message);
    }

    /// The output tier is one param on one scale for every vendor: checked
    /// against the model's `resolution` values, and Google's former
    /// `extra.imageSize` (and any `extra.resolution`) is told where it goes.
    #[test]
    fn an_image_resolution_is_one_of_the_models_tiers() {
        use serde_json::json;
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.resolution = Some("2K".into());
        assert!(validate_image(m, req).is_ok());
        let mut req = img("hi");
        req.resolution = Some("2k".into());
        let err = validate_image(m, req).unwrap_err();
        assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_enum_mismatch", Some("resolution")), "{}", err.message);
        for extra in [json!({ "imageSize": "2K" }), json!({ "resolution": "2K" })] {
            let mut req = img("hi");
            req.base.extra = Some(extra.clone());
            let err = validate_image(m, req).unwrap_err();
            assert!(err.message.contains("send it as `resolution`"), "{extra}: {}", err.message);
        }
    }

    /// Lax mode drops the `extra` keys a model does not allowlist, and says
    /// so, as it does for params. Passed through, they reached the vendor and
    /// could replace what litegen sends: `extra.model` ran a dearer model than
    /// the one billed (vendor audits, 2026-10-30).
    #[test]
    fn extra_lax_keeps_only_allowlisted_keys_and_reports_the_rest() {
        use serde_json::json;
        let r = registry();
        let m = r.get("t/strict").unwrap();
        let mut req = img("hi");
        req.base.strict = false;
        req.base.extra = Some(json!({"output_format": "png", "ghost": 1}));
        let out = validate_image(m, req).unwrap();
        assert_eq!(out.request.base.extra, Some(json!({"output_format": "png"})));
        assert!(out.dropped.iter().any(|d| d == "extra.ghost"), "{:?}", out.dropped);
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
  - id: v/open-float
    provider: v
    media_type: video
    display_name: OpenFloat
    pricing: { base_cost_usd: 0.5 }
    capabilities: { text_to_video: true }
    prompt: { required: true, max_length: 100 }
    params:
      duration_seconds: { kind: float, max: 10.0 }
  - id: v/open-int
    provider: v
    media_type: video
    display_name: OpenInt
    pricing: { base_cost_usd: 0.5 }
    capabilities: { text_to_video: true }
    prompt: { required: true, max_length: 100 }
    params:
      duration_seconds: { kind: int, max: 10 }
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

    /// A vendor that takes whole seconds makes the rounded length, so the
    /// request carries the rounded length on: the price and the vendor see
    /// the same seconds (C5) — never 8.4 s billed for an 8 s clip.
    #[test]
    fn a_whole_second_duration_is_rounded_for_the_price_and_the_vendor_alike() {
        let r = video_registry();
        let mut req = vid_json("v/eight");
        req.duration_seconds = Some(8.4);
        let out = validate_video(r.get("v/eight").unwrap(), req).unwrap();
        assert_eq!(out.request.duration_seconds, Some(8.0));
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

    /// A clip of no length, or less, is a bad value in either mode (E1): it
    /// used to skip every check and be priced at 0 s while the vendor made,
    /// and billed, its default clip. The open rows have no `min`, so the range
    /// check alone cannot refuse it there.
    #[test]
    fn a_duration_of_zero_or_less_is_refused_in_both_modes() {
        let r = video_registry();
        let mut let_through = Vec::new();
        for model in ["v/six", "v/eight", "v/open-float", "v/open-int"] {
            for d in [0.0, -0.0, -3.0, -1e300] {
                for strict in [true, false] {
                    let mut req = vid_json(model);
                    req.duration_seconds = Some(d);
                    req.base.strict = strict;
                    let out = validate_video(r.get(model).unwrap(), req);
                    let refused = matches!(&out, Err(e) if e.code == "param_out_of_range" && e.param.as_deref() == Some("duration_seconds"));
                    if !refused {
                        let_through.push(format!("{model} {d:?} strict={strict}: {out:?}"));
                    }
                }
            }
        }
        assert!(let_through.is_empty(), "{} durations of 0 s or less were not refused:\n{}", let_through.len(), let_through.join("\n"));
    }

    /// The engine's backstop: a duration of 0 s or less (or not a number)
    /// never resolves to itself, so a path that skips the validator prices the
    /// model's default — the clip the vendor makes — never a $0 one.
    #[test]
    fn a_non_positive_duration_never_resolves_to_a_clip_of_no_length() {
        let r = video_registry();
        let six = r.get("v/six").unwrap();
        for d in [0.0, -0.0, -3.0, f64::NAN, f64::INFINITY] {
            let got = resolve_duration_seconds(six, Some(d));
            assert!(got == 6.0, "v/six with {d} resolved to {got}, not its 6 s default");
        }
        let got = resolve_duration_seconds(r.get("v/none").unwrap(), Some(0.0));
        assert!(got == 5.0, "v/none with 0 resolved to {got}, not the legacy 5 s");
    }

    /// A model whose listing has no `duration_seconds` takes none (E6): sent
    /// anyway, it is refused when strict and dropped and named otherwise, like
    /// every other param the model lacks.
    #[test]
    fn a_duration_on_a_model_without_one_is_refused_in_strict_and_dropped_in_lax() {
        let r = video_registry();
        let none = r.get("v/none").unwrap();
        let mut req = vid_json("v/none");
        req.duration_seconds = Some(7.0);
        let out = validate_video(none, req);
        assert!(
            matches!(&out, Err(e) if e.code == "param_unsupported" && e.param.as_deref() == Some("duration_seconds")),
            "strict: {out:?}"
        );
        let mut req = vid_json("v/none");
        req.duration_seconds = Some(7.0);
        req.base.strict = false;
        let out = validate_video(none, req).expect("lax drops what the model lacks");
        assert_eq!((out.dropped.clone(), out.request.duration_seconds), (vec!["duration_seconds".to_string()], None), "lax");
    }

    // ─── Request-time checks (L13) ───────────────────────────────────────────

    fn l13_registry() -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("l13.yaml", r#"
models:
  - id: c/edit-only
    provider: c
    media_type: image
    display_name: EditOnly
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: false, image_to_image: true }
    prompt: { required: true }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: url }
      roles:
        init: { required: false, min_count: 0, max_count: 1 }
  - id: c/text-only
    provider: c
    media_type: image
    display_name: TextOnly
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    params:
      size: { kind: size, mode: freeform, min_width: 480, max_width: 3840, min_height: 480, max_height: 3840, multiple_of: 16, min_pixels: 655360, max_pixels: 8294400, max_aspect: 3.0 }
  - id: c/i2v-only
    provider: c
    media_type: video
    display_name: I2VOnly
    pricing: { base_cost_usd: 0.1 }
    capabilities: { image_to_video: true }
    prompt: { required: true }
    ref_inputs:
      max_total: 1
      default_role: first_frame
      provider_format: { form: url }
      roles:
        first_frame: { required: false, min_count: 0, max_count: 1 }
  - id: c/t2v-only
    provider: c
    media_type: video
    display_name: T2VOnly
    pricing: { base_cost_usd: 0.1 }
    capabilities: { text_to_video: true }
    prompt: { required: true }
    ref_inputs:
      max_total: 1
      default_role: first_frame
      provider_format: { form: url }
      roles:
        first_frame: { required: false, min_count: 0, max_count: 1 }
"#)]).unwrap()
    }

    fn img_req(model: &str, refs: serde_json::Value, extra: serde_json::Value) -> ImageGenerationRequest {
        let mut body = serde_json::json!({ "model": model, "prompt": "a fox", "reference_images": refs });
        for (k, v) in extra.as_object().unwrap() {
            body[k] = v.clone();
        }
        serde_json::from_value(body).unwrap()
    }

    #[test]
    fn a_capability_the_model_lacks_is_refused_by_name() {
        let r = l13_registry();
        let err = validate_image(r.get("c/edit-only").unwrap(), img_req("c/edit-only", serde_json::json!([]), serde_json::json!({}))).unwrap_err();
        assert_eq!(err.code, "capability_unsupported", "{}", err.message);
        let frame = serde_json::json!([{ "type": "url", "value": "https://x/a.png", "role": "first_frame" }]);
        let video = |model: &str, refs: serde_json::Value| -> VideoGenerationRequest {
            serde_json::from_value(serde_json::json!({ "model": model, "prompt": "a fox", "reference_images": refs })).unwrap()
        };
        let err = validate_video(r.get("c/i2v-only").unwrap(), video("c/i2v-only", serde_json::json!([]))).unwrap_err();
        assert_eq!(err.code, "capability_unsupported", "{}", err.message);
        let err = validate_video(r.get("c/t2v-only").unwrap(), video("c/t2v-only", frame.clone())).unwrap_err();
        assert_eq!(err.code, "capability_unsupported", "{}", err.message);
        assert!(validate_video(r.get("c/i2v-only").unwrap(), video("c/i2v-only", frame)).is_ok());
    }

    #[test]
    fn more_than_one_image_per_request_is_refused() {
        let r = l13_registry();
        let err = validate_image(r.get("c/text-only").unwrap(), img_req("c/text-only", serde_json::json!([]), serde_json::json!({ "n": 2 }))).unwrap_err();
        assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_out_of_range", Some("n")), "{}", err.message);
        assert!(validate_image(r.get("c/text-only").unwrap(), img_req("c/text-only", serde_json::json!([]), serde_json::json!({ "n": 1 }))).is_ok());
    }

    /// One submitted job makes one clip, and one is billed (E3): `n` above 1
    /// was accepted and silently ignored. Refused in both modes, as an image's
    /// `n` is; 0 is the same as 1.
    #[test]
    fn more_than_one_video_per_request_is_refused() {
        let r = video_registry();
        let six = r.get("v/six").unwrap();
        for strict in [true, false] {
            let mut req = vid_json("v/six");
            req.base.n = 4;
            req.base.strict = strict;
            let out = validate_video(six, req);
            assert!(matches!(&out, Err(e) if e.code == "param_out_of_range" && e.param.as_deref() == Some("n")), "n: 4, strict={strict}: {out:?}");
        }
        for n in [0, 1] {
            let mut req = vid_json("v/six");
            req.base.n = n;
            let out = validate_video(six, req);
            assert!(out.is_ok(), "n: {n}: {out:?}");
        }
    }

    #[test]
    fn a_size_the_vendor_cannot_make_is_refused_not_snapped() {
        let r = l13_registry();
        let m = r.get("c/text-only").unwrap();
        for size in ["480x480", "3840x3840", "3840x1024"] {
            let err = validate_image(m, img_req("c/text-only", serde_json::json!([]), serde_json::json!({ "size": size }))).unwrap_err();
            assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_out_of_range", Some("size")), "{size}: {}", err.message);
        }
        assert!(validate_image(m, img_req("c/text-only", serde_json::json!([]), serde_json::json!({ "size": "2160x3840" }))).is_ok());
    }

    // ─── A frame image sets the clip's shape (E5) ────────────────────────────

    /// Rows shaped like `c/i2v-only` whose vendor takes no ratio once a frame
    /// image sets the clip's shape: `f/framed` unless a `reference` image
    /// rides along (a mode that takes one), `f/always` even then, and
    /// `f/plain` never (the ratio always applies).
    fn framed_registry() -> CapabilityRegistry {
        let row = |id: &str, shape: &str| format!(r#"
  - id: {id}
    provider: f
    media_type: video
    display_name: Framed
    pricing: {{ base_cost_usd: 0.1 }}
    capabilities: {{ text_to_video: true, image_to_video: true }}
    prompt: {{ required: true }}
    params:
      aspect_ratio: {{ kind: aspect_ratio, allowed: ["16:9", "9:16"]{shape} }}
    ref_inputs:
      max_total: 2
      default_role: first_frame
      provider_format: {{ form: url }}
      roles:
        first_frame: {{ required: false, min_count: 0, max_count: 1 }}
        last_frame: {{ required: false, min_count: 0, max_count: 1 }}
        init: {{ required: false, min_count: 0, max_count: 1 }}
        reference: {{ required: false, min_count: 0, max_count: 1 }}"#);
        let yaml = format!(
            "models:{}{}{}\n",
            row("f/framed", ", frames_set_shape: without_references"),
            row("f/always", ", frames_set_shape: always"),
            row("f/plain", ""),
        );
        CapabilityRegistry::from_yaml_strs(&[("f.yaml", yaml.as_str())]).unwrap()
    }

    /// A validated `aspect_ratio` the vendor would not get (E5): the adapter
    /// left it out of the image-to-video body, so a 9:16 request with a 16:9
    /// frame came back 16:9 with nothing refused or reported. Refused when
    /// strict, dropped and named when lax; kept wherever it is sent.
    #[test]
    fn a_ratio_a_frame_overrides_is_refused_in_strict_and_dropped_in_lax() {
        let r = framed_registry();
        let run = |model: &str, roles: &[Option<&str>], strict: bool| {
            let refs: Vec<serde_json::Value> = roles
                .iter()
                .enumerate()
                .map(|(i, role)| {
                    let mut image = serde_json::json!({ "type": "url", "value": format!("https://x/{i}.png") });
                    if let Some(role) = role {
                        image["role"] = serde_json::json!(role);
                    }
                    image
                })
                .collect();
            let req: VideoGenerationRequest = serde_json::from_value(serde_json::json!({
                "model": model, "prompt": "a fox", "aspect_ratio": "9:16", "reference_images": refs, "strict": strict,
            }))
            .unwrap();
            validate_video(r.get(model).unwrap(), req)
        };
        // Strict: a frame image, named or role-less (its `default_role`), refuses the ratio.
        for (model, roles) in [
            ("f/framed", vec![Some("first_frame")]),
            ("f/framed", vec![None]),
            ("f/framed", vec![Some("last_frame")]),
            ("f/framed", vec![Some("init")]),
            ("f/always", vec![Some("first_frame")]),
            ("f/always", vec![Some("first_frame"), Some("reference")]),
        ] {
            let out = run(model, &roles, true);
            assert!(
                matches!(&out, Err(e) if e.code == "param_unsupported" && e.param.as_deref() == Some("aspect_ratio")),
                "{model} {roles:?} strict: {out:?}"
            );
        }
        // Lax: dropped and named; the image still goes.
        let out = run("f/framed", &[Some("first_frame")], false).expect("lax drops the ratio");
        assert_eq!(
            (out.dropped.clone(), out.request.aspect_ratio.clone(), out.request.base.reference_images.len()),
            (vec!["aspect_ratio".to_string()], None, 1),
            "lax"
        );
        // Kept where the vendor takes it: no frame, a reference beside the frame, or no rule.
        for (model, roles) in [
            ("f/framed", vec![]),
            ("f/framed", vec![Some("first_frame"), Some("reference")]),
            ("f/framed", vec![Some("reference")]),
            ("f/always", vec![Some("reference")]),
            ("f/plain", vec![Some("first_frame")]),
        ] {
            for strict in [true, false] {
                let out = run(model, &roles, strict);
                assert!(
                    matches!(&out, Ok(o) if o.request.aspect_ratio.as_deref() == Some("9:16") && o.dropped.is_empty()),
                    "{model} {roles:?} strict={strict}: {out:?}"
                );
            }
        }
    }

    // ─── Fields the request has no name for (E2) ─────────────────────────────

    fn allow_registry() -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("u.yaml", r#"
models:
  - id: u/allow
    provider: u
    media_type: image
    display_name: Allow
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    params: {}
    extra_allowlist: [user, moderation]
"#)]).unwrap()
    }

    fn unknown(
        m: &ModelSchema,
        request: &str,
        top: Vec<(&str, serde_json::Value)>,
        nested: Vec<&str>,
        extra: &mut Option<serde_json::Value>,
        strict: bool,
    ) -> Result<Vec<String>, ValidationError> {
        let top = top.into_iter().map(|(k, v)| (k.to_string(), v)).collect();
        check_unknown_fields(m, request, top, nested.into_iter().map(str::to_string).collect(), extra, strict)
    }

    /// Strict refuses a field the request type has no name for, at any depth,
    /// naming it; lax drops it and names it for `X-Litegen-Dropped-Params`.
    /// Both used to be ignored, `strict: true` included.
    #[test]
    fn an_unknown_field_is_refused_in_strict_and_named_in_lax() {
        let r = allow_registry();
        let m = r.get("u/allow").unwrap();
        let img = "ImageGenerationRequest";
        let out = unknown(m, img, vec![("colour", serde_json::json!("red"))], vec![], &mut None, true);
        assert!(matches!(&out, Err(e) if e.code == "param_unsupported" && e.param.as_deref() == Some("colour")), "strict, top level: {out:?}");
        let out = unknown(m, img, vec![], vec!["reference_images[0].weight"], &mut None, true);
        assert!(
            matches!(&out, Err(e) if e.code == "param_unsupported" && e.param.as_deref() == Some("reference_images[0].weight")),
            "strict, nested: {out:?}"
        );
        let out = unknown(m, img, vec![("colour", serde_json::json!("red"))], vec!["reference_images[0].weight"], &mut None, false);
        assert_eq!(out.ok(), Some(vec!["colour".to_string(), "reference_images[0].weight".to_string()]), "lax names both");
    }

    /// A field the model passes through (`extra_allowlist`) is forwarded as if
    /// sent in `extra`, in both modes (E2 ruling a): the OpenAI SDK sends
    /// `user` at the top level. Sent both ways with different values, strict
    /// refuses naming both, and lax keeps `extra`'s and names the other.
    #[test]
    fn a_field_the_model_passes_through_is_forwarded_as_extra() {
        let r = allow_registry();
        let m = r.get("u/allow").unwrap();
        let img = "ImageGenerationRequest";
        let user = || vec![("user", serde_json::json!("u1"))];
        for strict in [true, false] {
            let mut extra = None;
            let out = unknown(m, img, user(), vec![], &mut extra, strict);
            assert_eq!((out.ok(), extra), (Some(vec![]), Some(serde_json::json!({ "user": "u1" }))), "strict={strict}");
            let mut extra = Some(serde_json::json!({ "user": "u1", "moderation": "low" }));
            let out = unknown(m, img, user(), vec![], &mut extra, strict);
            assert_eq!(
                (out.ok(), extra),
                (Some(vec![]), Some(serde_json::json!({ "user": "u1", "moderation": "low" }))),
                "the same value both ways, strict={strict}"
            );
        }
        let mut extra = Some(serde_json::json!({ "user": "u2" }));
        let out = unknown(m, img, user(), vec![], &mut extra, true);
        assert!(
            matches!(&out, Err(e) if e.code == "param_invalid" && e.param.as_deref() == Some("user") && e.message.contains("extra.user")),
            "strict, two values: {out:?}"
        );
        let mut extra = Some(serde_json::json!({ "user": "u2" }));
        let out = unknown(m, img, user(), vec![], &mut extra, false);
        assert_eq!((out.ok(), extra), (Some(vec!["user".to_string()]), Some(serde_json::json!({ "user": "u2" }))), "lax keeps extra's value");
    }

    /// Strict names at most 20 fields and a count (a 25 MiB body of tiny keys
    /// would make a megabyte error otherwise), with a hint where one fits.
    #[test]
    fn a_refusal_names_twenty_fields_and_hints_at_the_right_one() {
        let r = allow_registry();
        let m = r.get("u/allow").unwrap();
        let top: Vec<(String, serde_json::Value)> = (0..25).map(|i| (format!("k{i:02}"), serde_json::json!(1))).collect();
        let err = check_unknown_fields(m, "ImageGenerationRequest", top, vec![], &mut None, true).unwrap_err();
        assert!(err.message.contains("k19") && !err.message.contains("k20") && err.message.contains("5 more"), "{}", err.message);
        let err = unknown(m, "VideoGenerationRequest", vec![("duration", serde_json::json!(10))], vec![], &mut None, true).unwrap_err();
        assert!(err.message.contains("`duration_seconds`"), "a field it begins: {}", err.message);
        let err = unknown(m, "VideoGenerationRequest", vec![("durationSeconds", serde_json::json!(10))], vec![], &mut None, true).unwrap_err();
        assert!(err.message.contains("`duration_seconds`"), "its snake_case form: {}", err.message);
        let err = unknown(m, "VideoGenerationRequest", vec![("size", serde_json::json!("1024x1024"))], vec![], &mut None, true).unwrap_err();
        assert!(err.message.contains("an image request field"), "another family's field: {}", err.message);
    }

    /// One name that is no header value used to delete the whole report
    /// (N2); a capped list stays visible ASCII, since the MCP note and the
    /// SDKs read the header with `to_str()`.
    #[test]
    fn the_dropped_header_survives_a_name_that_is_no_header_value() {
        let names = |v: &[&str]| v.iter().map(|s| s.to_string()).collect::<Vec<_>>();
        let out = dropped_header(&names(&["fps", "extra.a\u{1}b", "extra.a,b"]));
        let (name, value) = out.clone().unwrap_or_else(|| panic!("one unsafe name deleted the report: {out:?}"));
        assert_eq!(name.as_str(), "x-litegen-dropped-params");
        assert_eq!(value.to_str().ok(), Some("fps,+2 more"));
        let many: Vec<String> = (0..40).map(|i| format!("p{i:02}")).chain(["extra.h\u{e9}llo".to_string(), "x".repeat(65)]).collect();
        let (_, value) = dropped_header(&many).expect("a capped header");
        let text = value.to_str().expect("visible ASCII");
        assert!(text.starts_with("p00,p01,") && text.ends_with(",p31,+10 more"), "{text}");
        assert!(dropped_header(&[]).is_none());
    }

    // ─── Timed keyframes (3D doc §5) ─────────────────────────────────────────

    fn keyframe_model(timing: &str, step: &str, duration: &str) -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("k.yaml", format!(r#"
models:
  - id: k/m
    provider: k
    media_type: video
    display_name: K
    pricing: {{ base_cost_usd: 0.1 }}
    capabilities: {{ image_to_video: true }}
    prompt: {{ required: true }}
    params:
{duration}
    ref_inputs:
      max_total: 8
      provider_format: {{ form: url }}
      roles:
        keyframe: {{ required: false, min_count: 0, max_count: 8, timing: {timing}{step} }}
"#).as_str())]).unwrap()
    }

    fn keyframed(times: &[Option<f64>]) -> VideoGenerationRequest {
        let mut req = vid_json("k/m");
        req.duration_seconds = Some(4.0);
        req.base.reference_images = times
            .iter()
            .enumerate()
            .map(|(i, t)| ReferenceImage { kind: RefImageKind::Url, value: format!("https://x/{i}.png"), role: Some("keyframe".into()), time_s: *t })
            .collect();
        req
    }

    const DURATION: &str = "      duration_seconds: { kind: float, min: 1.0, max: 10.0, default: 5.0 }";

    #[test]
    fn keyframes_reach_the_vendor_in_time_order() {
        let r = keyframe_model("any", "", DURATION);
        let out = validate_video(r.get("k/m").unwrap(), keyframed(&[Some(3.0), Some(0.0), Some(1.25)])).unwrap();
        let order: Vec<(String, Option<f64>)> = out.request.base.reference_images.iter().map(|r| (r.value.clone(), r.time_s)).collect();
        assert_eq!(order, [("https://x/1.png".into(), Some(0.0)), ("https://x/2.png".into(), Some(1.25)), ("https://x/0.png".into(), Some(3.0))]);
    }

    #[test]
    fn even_keyframes_are_spaced_by_litegen_or_must_already_be_even() {
        let r = keyframe_model("even", "", DURATION);
        let out = validate_video(r.get("k/m").unwrap(), keyframed(&[None, None, None])).unwrap();
        let times: Vec<Option<f64>> = out.request.base.reference_images.iter().map(|r| r.time_s).collect();
        assert_eq!(times, [Some(0.0), Some(2.0), Some(4.0)], "start to end of the 4 s clip");
        assert!(validate_video(r.get("k/m").unwrap(), keyframed(&[Some(0.0), Some(2.0), Some(4.0)])).is_ok());
        let err = validate_video(r.get("k/m").unwrap(), keyframed(&[Some(0.0), Some(1.0), Some(4.0)])).unwrap_err();
        assert_eq!(err.code, "keyframe_timing_uneven", "{}", err.message);
    }

    #[test]
    fn a_keyframe_off_the_vendors_grid_is_refused() {
        let r = keyframe_model("any", ", time_step_s: 1.0", "      {}");
        // A model without a duration param takes none (E6): its keyframes make the clip.
        let unbounded = |times: &[Option<f64>]| VideoGenerationRequest { duration_seconds: None, ..keyframed(times) };
        assert!(validate_video(r.get("k/m").unwrap(), unbounded(&[Some(0.0), Some(2.0), Some(7.0)])).is_ok(), "no duration param: the keyframes make the clip");
        let err = validate_video(r.get("k/m").unwrap(), unbounded(&[Some(0.0), Some(1.5)])).unwrap_err();
        assert_eq!(err.code, "keyframe_off_grid", "{}", err.message);
    }

    // ─── Prompt and required-image rules shared by every family ─────────────

    fn edit_model(init_role: &str, prompt: &str) -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("x.yaml", format!(r#"
models:
  - id: t/edit
    provider: t
    media_type: image
    display_name: Edit
    pricing: {{ base_cost_usd: 0.01 }}
    capabilities: {{ image_to_image: true }}
    prompt: {prompt}
    ref_inputs:
      max_total: 3
      default_role: init
      provider_format: {{ form: url }}
      roles:
        init: {init_role}
"#).as_str())]).unwrap()
    }

    fn with_images(n: usize) -> ImageGenerationRequest {
        let mut r = img("hi");
        r.base.model = "t/edit".into();
        r.base.reference_images = (0..n)
            .map(|i| ReferenceImage { kind: RefImageKind::Url, value: format!("https://x/{i}.png"), role: Some("init".into()), time_s: None })
            .collect();
        r
    }

    #[test]
    fn a_missing_required_image_is_reported_as_required_not_as_a_count() {
        // `min_count: 1` is the usual spelling; with `min_count: 0` the role
        // used to be declared required and never enforced at all.
        for min in [1, 0] {
            let r = edit_model(&format!("{{ required: true, min_count: {min}, max_count: 3 }}"), "{ required: true }");
            let m = r.get("t/edit").unwrap();
            let err = validate_image(m, with_images(0)).unwrap_err();
            assert_eq!(err.code, "ref_role_required", "min_count {min}: {}", err.message);
            assert!(validate_image(m, with_images(1)).is_ok(), "min_count {min}");
        }
    }

    #[test]
    fn too_few_images_for_a_role_is_still_a_count_error() {
        let r = edit_model("{ required: true, min_count: 2, max_count: 3 }", "{ required: true }");
        let m = r.get("t/edit").unwrap();
        assert_eq!(validate_image(m, with_images(1)).unwrap_err().code, "ref_role_count_out_of_range");
        assert!(validate_image(m, with_images(2)).is_ok());
    }

    #[test]
    fn a_model_with_no_prompt_field_refuses_one_in_strict_and_drops_it_in_lax() {
        let r = edit_model("{ required: true, min_count: 1, max_count: 1 }", "{ required: false, max_length: 0 }");
        let m = r.get("t/edit").unwrap();

        let err = validate_image(m, with_images(1)).unwrap_err();
        assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_unsupported", Some("prompt")));

        let mut lax = with_images(1);
        lax.base.strict = false;
        let out = validate_image(m, lax).unwrap();
        assert_eq!(out.dropped, vec!["prompt".to_string()]);
        assert!(out.request.base.prompt.is_empty());

        let mut none = with_images(1);
        none.base.prompt = String::new();
        let out = validate_image(m, none).unwrap();
        assert!(out.dropped.is_empty(), "an absent prompt is not a dropped one");
    }

    #[test]
    fn prompt_length_counts_characters_not_bytes() {
        // Vendor limits come from JSON Schema `maxLength`, which counts code
        // points; "é" is two bytes.
        let r = registry();
        let m = r.get("t/strict").unwrap();
        assert!(validate_image(m, img(&"é".repeat(100))).is_ok());
        assert_eq!(validate_image(m, img(&"é".repeat(101))).unwrap_err().code, "prompt_too_long");
    }

    // ─── Segmentation (PixelGenie L7) ─────────────────────────────────────────

    fn segmenter(types: &str) -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("t.yaml", &format!(r#"
models:
  - id: t/seg
    provider: t
    media_type: image
    task: segment
    display_name: Seg
    pricing: {{ base_cost_usd: 0.005 }}
    capabilities: {{}}
    prompt: {{ required: false, max_length: 0 }}
    params:
      prompts: {{ kind: segmentation_prompts, types: {types}, max_items: 3, max_masks: 8 }}
"#))]).unwrap()
    }

    fn seg(prompts: serde_json::Value) -> ImageSegmentationRequest {
        serde_json::from_value(serde_json::json!({ "model": "t/seg", "image": "", "prompts": prompts })).unwrap()
    }

    #[test]
    fn segmentation_prompts_are_checked_against_the_model_before_billing() {
        use serde_json::json;
        let visual = segmenter("[point, box]");
        let all = segmenter("[point, box, text]");
        let (visual, all) = (visual.get("t/seg").unwrap(), all.get("t/seg").unwrap());
        assert!(validate_segmentation(visual, &seg(json!([{ "type": "box", "x": 0, "y": 0, "w": 4, "h": 4 }, { "type": "point", "x": 1, "y": 1, "label": 0 }]))).is_ok());
        assert!(validate_segmentation(all, &seg(json!([{ "type": "text", "text": "visor" }]))).is_ok());
        for (m, prompts, code) in [
            (visual, json!([]), "param_required"),
            (visual, json!([{ "type": "text", "text": "visor" }]), "capability_unsupported"),
            (visual, json!([{ "type": "point", "x": 1, "y": 1 }, { "type": "point", "x": 2, "y": 1 }, { "type": "point", "x": 3, "y": 1 }, { "type": "point", "x": 4, "y": 1 }]), "param_out_of_range"),
            (all, json!([{ "type": "text", "text": "visor" }, { "type": "box", "x": 0, "y": 0, "w": 4, "h": 4 }]), "param_conflict"),
            (all, json!([{ "type": "text", "text": "a" }, { "type": "text", "text": "b" }]), "param_out_of_range"),
            (all, json!([{ "type": "text", "text": "  " }]), "param_required"),
            (all, json!([{ "type": "point", "x": 1, "y": 1, "label": 2 }]), "param_out_of_range"),
            (all, json!([{ "type": "box", "x": 1, "y": 1, "w": 4, "h": 0 }]), "param_out_of_range"),
            (all, json!([{ "type": "point", "x": 1, "y": 1, "label": 0 }]), "param_conflict"),
        ] {
            let err = validate_segmentation(m, &seg(prompts.clone())).unwrap_err();
            assert_eq!((err.code.as_str(), err.param.as_deref()), (code, Some("prompts")), "{prompts}: {}", err.message);
        }
    }

    #[test]
    fn segmentation_prompts_must_fall_inside_the_image() {
        use serde_json::json;
        let inside = seg(json!([{ "type": "box", "x": 6, "y": 0, "w": 4, "h": 10 }, { "type": "point", "x": 9, "y": 9 }, { "type": "text", "text": "x" }])).prompts;
        assert!(check_segmentation_bounds(&inside, 10, 10).is_ok());
        for prompt in [json!({ "type": "box", "x": 7, "y": 0, "w": 4, "h": 10 }), json!({ "type": "point", "x": 0, "y": 10 })] {
            let err = check_segmentation_bounds(&seg(json!([prompt])).prompts, 10, 10).unwrap_err();
            assert_eq!((err.code.as_str(), err.param.as_deref()), ("prompt_out_of_bounds", Some("prompts")), "{prompt}");
        }
    }

    #[test]
    fn a_segmentation_model_is_served_only_where_segmentations_are() {
        let r = segmenter("[point]");
        let m = r.get("t/seg").unwrap();
        assert_eq!(require_task(m, ModelTask::Generate).unwrap_err().code, "model_task_mismatch");
        assert!(require_task(m, ModelTask::Segment).is_ok());
        let plain = registry();
        let err = require_task(plain.get("t/strict").unwrap(), ModelTask::Segment).unwrap_err();
        assert!(err.message.contains("/v1/images/generations"), "{}", err.message);
    }

    // ─── Expand (3D doc §4, P2) ───────────────────────────────────────────────

    #[test]
    fn a_vendor_that_only_centres_takes_only_symmetric_expansions() {
        let r = CapabilityRegistry::from_yaml_strs(&[("t.yaml", r#"
models:
  - id: t/centred
    provider: t
    media_type: image
    display_name: Centred
    pricing: { base_cost_usd: 0.01 }
    capabilities: { image_to_image: true }
    prompt: { required: false, max_length: 100 }
    params:
      expand: { kind: expand, centered_only: true, max_px: 100 }
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format: { form: base64 }
      roles:
        init: { required: true, min_count: 1, max_count: 1 }
"#)]).unwrap();
        let m = r.get("t/centred").unwrap();
        let ask = |l: u32, t: u32, rt: u32, b: u32| {
            let mut req = with_images(1);
            req.base.model = "t/centred".into();
            req.base.prompt = String::new();
            req.expand = Some(Expand { left: l, top: t, right: rt, bottom: b });
            validate_image(m, req)
        };
        assert!(ask(10, 4, 10, 4).is_ok());
        let err = ask(10, 0, 0, 0).unwrap_err();
        assert_eq!((err.code.as_str(), err.param.as_deref()), ("expand_unsupported", Some("expand")));
    }

    // ─── Masks named in text (3D doc §4, P2) ──────────────────────────────────

    #[test]
    fn a_mask_named_in_text_cannot_ride_with_a_drawn_one() {
        let r = CapabilityRegistry::from_yaml_strs(&[("t.yaml", r#"
models:
  - id: t/both
    provider: t
    media_type: image
    display_name: Both
    pricing: { base_cost_usd: 0.01 }
    capabilities: { image_to_image: true, inpainting: true }
    prompt: { required: true, max_length: 100 }
    params:
      mask_prompt: { kind: string, max_length: 50 }
      region: { kind: region, input: mask }
    ref_inputs:
      max_total: 2
      default_role: init
      provider_format: { form: base64 }
      roles:
        init: { required: true, min_count: 1, max_count: 1 }
        mask: { required: false, min_count: 0, max_count: 1, convention: white_edits }
"#)]).unwrap();
        let m = r.get("t/both").unwrap();
        let mut req = with_images(1);
        req.base.model = "t/both".into();
        req.mask_prompt = Some("the door".into());
        assert!(validate_image(m, req.clone()).is_ok());
        req.region = Some(Region { x: 0, y: 0, w: 4, h: 4 });
        let err = validate_image(m, req.clone()).unwrap_err();
        assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_conflict", Some("mask_prompt")));
        req.region = None;
        req.mask_prompt = Some("x".repeat(51));
        assert_eq!(validate_image(m, req).unwrap_err().param.as_deref(), Some("mask_prompt"), "the model's own length limit");
    }

    // ─── Video operations (3D doc §5, P2) ─────────────────────────────────────

    #[test]
    fn a_window_off_a_whole_second_grid_is_refused_where_the_model_keeps_one() {
        let r = CapabilityRegistry::from_yaml_strs(&[("t.yaml", r#"
models:
  - id: t/retake
    provider: t
    media_type: video
    task: retake
    display_name: Retake
    pricing: { base_cost_usd: 0.01 }
    capabilities: {}
    prompt: { required: true, max_length: 100 }
    params:
      source_video: { kind: source_video, formats: [mp4] }
      time_range: { kind: time_range, min_window_s: 2, whole_seconds: true }
"#)]).unwrap();
        let m = r.get("t/retake").unwrap();
        let ask = |start_s: f64, end_s: f64| {
            let req: VideoGenerationRequest = serde_json::from_value(serde_json::json!({
                "model": "t/retake", "prompt": "x",
                "source_video": { "type": "url", "value": "https://example.com/a.mp4", "format": "mp4" },
                "time_range": { "start_s": start_s, "end_s": end_s },
            }))
            .unwrap();
            validate_video(m, req)
        };
        assert!(ask(1.0, 3.0).is_ok());
        for (s, e) in [(1.5, 3.5), (1.0, 2.0), (3.0, 1.0)] {
            let err = ask(s, e).unwrap_err();
            assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_out_of_range", Some("time_range")), "{s}–{e}");
        }
        let req: VideoGenerationRequest = serde_json::from_value(serde_json::json!({
            "model": "t/retake", "prompt": "x", "time_range": { "start_s": 1.0, "end_s": 3.0 },
            "source_video": { "type": "url", "value": "https://example.com/a.webm", "format": "webm" },
        }))
        .unwrap();
        assert_eq!(validate_video(m, req).unwrap_err().code, "param_enum_mismatch", "a container the model does not take");
    }

    // ─── A `values` list is enforced on every number param ───────────────────

    fn listed_registry() -> CapabilityRegistry {
        CapabilityRegistry::from_yaml_strs(&[("listed.yaml", r#"
models:
  - id: n/img
    provider: n
    media_type: image
    display_name: Img
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true, max_length: 100 }
    params:
      steps: { kind: int, min: 1, max: 50, values: [10, 20] }
      guidance_scale: { kind: float, min: 1.0, max: 10.0, values: [3.5, 7.0] }
      strength: { kind: float, min: 0.0, max: 1.0, values: [0.5, 1.0] }
  - id: n/vid
    provider: n
    media_type: video
    display_name: Vid
    pricing: { base_cost_usd: 0.5 }
    capabilities: { text_to_video: true }
    prompt: { required: true, max_length: 100 }
    params:
      fps: { kind: int, min: 24, max: 50, values: [24, 25, 48, 50] }
  - id: n/mesh
    provider: n
    media_type: model3d
    display_name: Mesh
    pricing: { base_cost_usd: 0.5 }
    capabilities: { text_to_3d: true }
    prompt: { required: true, max_length: 100 }
    params:
      output_formats: { kind: string_array, enum_values: [glb], max_items: 1, default: [glb] }
      target_polycount: { kind: int, min: 1000, max: 100000, values: [5000, 10000] }
      fps: { kind: int, min: 24, max: 60, values: [24, 30] }
"#)]).unwrap()
    }

    #[test]
    fn a_values_list_is_enforced_on_every_number_param() {
        let r = listed_registry();
        let img = |field: &str, v: serde_json::Value| -> Result<_, ValidationError> {
            let req: ImageGenerationRequest = serde_json::from_value(serde_json::json!({ "model": "n/img", "prompt": "a cat", field: v })).unwrap();
            validate_image(r.get("n/img").unwrap(), req)
        };
        for (field, bad, good) in [("steps", serde_json::json!(15), serde_json::json!(20)), ("guidance_scale", serde_json::json!(5.0), serde_json::json!(7.0)), ("strength", serde_json::json!(0.7), serde_json::json!(0.5))] {
            let err = img(field, bad).unwrap_err();
            assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_enum_mismatch", Some(field)), "{field}: {}", err.message);
            assert!(img(field, good).is_ok(), "{field}: a listed value passes");
        }

        let vid = |fps: u32| {
            let req: VideoGenerationRequest = serde_json::from_value(serde_json::json!({ "model": "n/vid", "prompt": "a cat", "fps": fps })).unwrap();
            validate_video(r.get("n/vid").unwrap(), req)
        };
        let err = vid(30).unwrap_err();
        assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_enum_mismatch", Some("fps")), "{}", err.message);
        assert!(err.message.contains("[24, 25, 48, 50]"), "the error names the values: {}", err.message);
        assert!(vid(48).is_ok());

        let mesh = |field: &str, v: u32| {
            let req: Model3dGenerationRequest = serde_json::from_value(serde_json::json!({ "model": "n/mesh", "prompt": "a chair", field: v })).unwrap();
            validate_model3d(r.get("n/mesh").unwrap(), req)
        };
        for (field, bad, good) in [("target_polycount", 7000, 10000), ("fps", 25, 30)] {
            let err = mesh(field, bad).unwrap_err();
            assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_enum_mismatch", Some(field)), "{field}: {}", err.message);
            assert!(mesh(field, good).is_ok(), "{field}: a listed value passes");
        }
    }

    #[test]
    fn a_start_frame_row_refuses_init_and_first_frame_together() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = CapabilityRegistry::from_dir(&p).unwrap();
        let req: VideoGenerationRequest = serde_json::from_value(serde_json::json!({
            "model": "runway/gen4.5",
            "prompt": "a drone shot",
            "reference_images": [
                { "type": "url", "value": "https://example.test/a.png", "role": "init" },
                { "type": "url", "value": "https://example.test/b.png", "role": "first_frame" }
            ]
        }))
        .unwrap();
        let err = validate_video(r.get("runway/gen4.5").unwrap(), req).unwrap_err();
        assert_eq!(err.code, "ref_total_exceeded", "{}", err.message);
    }

    /// N4: a string param's `pattern`, and a typed extra's, is enforced: a
    /// value it does not match is refused (`param_invalid`), in either mode.
    #[test]
    fn a_string_must_match_its_pattern() {
        use serde_json::json;
        let r = CapabilityRegistry::from_yaml_strs(&[("t.yaml", r#"
models:
  - id: t/pattern
    provider: t
    media_type: image
    display_name: T
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    params:
      style: { kind: string, pattern: "^[a-z]+$" }
      negative_prompt: { kind: string, pattern: "^[a-z ]*$" }
    extra_allowlist: [code]
    extra_params:
      code: { kind: string, pattern: "^[0-9a-f]{8}$" }
"#)])
        .expect("load");
        let m = r.get("t/pattern").unwrap();
        for strict in [true, false] {
            let mut req = img("hi");
            req.base.strict = strict;
            req.style = Some("ABC".into());
            let err = validate_image(m, req).expect_err("style");
            assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_invalid", Some("style")), "{}", err.message);
            let mut req = img("hi");
            req.base.strict = strict;
            req.base.negative_prompt = Some("NO".into());
            let err = validate_image(m, req).expect_err("negative_prompt");
            assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_invalid", Some("negative_prompt")), "{}", err.message);
            let mut req = img("hi");
            req.base.strict = strict;
            req.base.extra = Some(json!({ "code": "xyz" }));
            let err = validate_image(m, req).expect_err("extra.code");
            assert_eq!((err.code.as_str(), err.param.as_deref()), ("param_invalid", Some("extra.code")), "{}", err.message);
        }
        let mut req = img("hi");
        req.style = Some("abc".into());
        req.base.negative_prompt = Some("no blur".into());
        req.base.extra = Some(json!({ "code": "0a1b2c3d" }));
        validate_image(m, req).unwrap_or_else(|e| panic!("{}", e.message));
    }

    /// E4b: a typed extra is checked like a param, in either mode, with
    /// `param: "extra.<key>"`; an untyped one passes as any JSON, and `null`
    /// passes as given.
    #[test]
    fn a_typed_extra_is_checked_like_a_param() {
        use serde_json::json;
        let r = CapabilityRegistry::from_yaml_strs(&[("t.yaml", r#"
models:
  - id: t/typed
    provider: t
    media_type: image
    display_name: T
    pricing: { base_cost_usd: 0.01 }
    capabilities: { text_to_image: true }
    prompt: { required: true }
    extra_allowlist: [safety_tolerance, raw, steps_x, strength_x, styles, controls]
    extra_params:
      safety_tolerance: { kind: string, enum_values: ["1", "2", "3"], default: "2" }
      raw: { kind: bool }
      steps_x: { kind: int, min: 1, max: 50 }
      strength_x: { kind: float, min: 0, max: 1 }
      styles: { kind: string_array, enum_values: [a, b], max_items: 1 }
"#)])
        .expect("load");
        let m = r.get("t/typed").unwrap();
        let check = |extra: serde_json::Value, strict: bool| {
            let mut req = img("hi");
            req.base.strict = strict;
            req.base.extra = Some(extra);
            validate_image(m, req)
        };
        for (extra, code, param) in [
            (json!({ "safety_tolerance": 2 }), "param_invalid", "extra.safety_tolerance"),
            (json!({ "safety_tolerance": "9" }), "param_enum_mismatch", "extra.safety_tolerance"),
            (json!({ "raw": "true" }), "param_invalid", "extra.raw"),
            (json!({ "steps_x": 99 }), "param_out_of_range", "extra.steps_x"),
            (json!({ "steps_x": 1.5 }), "param_invalid", "extra.steps_x"),
            (json!({ "strength_x": 2.0 }), "param_out_of_range", "extra.strength_x"),
            (json!({ "strength_x": "0.5" }), "param_invalid", "extra.strength_x"),
            (json!({ "styles": ["c"] }), "param_enum_mismatch", "extra.styles"),
            (json!({ "styles": ["a", "b"] }), "param_too_many", "extra.styles"),
            (json!({ "styles": "a" }), "param_invalid", "extra.styles"),
            (json!({ "styles": [1] }), "param_invalid", "extra.styles"),
        ] {
            for strict in [true, false] {
                let err = check(extra.clone(), strict).expect_err(&extra.to_string());
                assert_eq!((err.code.as_str(), err.param.as_deref()), (code, Some(param)), "{extra} strict={strict}: {}", err.message);
            }
        }
        for extra in [
            json!({ "safety_tolerance": "2", "raw": true, "steps_x": 10, "strength_x": 0.5, "styles": ["a"] }),
            json!({ "controls": { "colors": [[1, 2, 3]] } }),
            json!({ "safety_tolerance": null }),
        ] {
            let out = check(extra.clone(), true).unwrap_or_else(|e| panic!("{extra}: {}", e.message));
            assert_eq!(out.request.base.extra.as_ref(), Some(&extra), "sent as given");
        }
    }

    /// E4a: where a result goes and who sees it are litegen's. The shipped
    /// rows that once allowlisted a vendor's callback or publish switch refuse
    /// it when strict, saying why, and drop and name it otherwise.
    #[test]
    fn a_caller_cannot_route_results_or_publish_them_through_extra() {
        use serde_json::json;
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let r = CapabilityRegistry::from_dir(&p).unwrap();
        let frame = json!([{ "type": "url", "value": "https://example.test/a.png", "role": "first_frame" }]);
        for (model, key, value, why, refs) in [
            ("vidu/viduq3-turbo", "callback_url", json!("https://attacker.example/cb"), "webhook", json!([])),
            ("minimax/MiniMax-Hailuo-2.3", "callback_url", json!("https://attacker.example/cb"), "webhook", json!([])),
            ("luma/dream-machine", "callback_url", json!("https://attacker.example/cb"), "webhook", json!([])),
            ("bytedance/doubao-seedance-1-0-pro-250528", "callback_url", json!("https://attacker.example/cb"), "webhook", json!([])),
            ("leonardo/motion2", "isPublic", json!(true), "access", frame),
        ] {
            let req = |strict: bool| -> VideoGenerationRequest {
                serde_json::from_value(json!({
                    "model": model, "prompt": "a cat", "strict": strict, "reference_images": refs.clone(), "extra": { key: value.clone() }
                }))
                .unwrap()
            };
            let err = validate_video(r.get(model).unwrap(), req(true)).expect_err(model);
            let param = format!("extra.{key}");
            assert_eq!((err.code.as_str(), err.param.as_deref()), ("extra_key_unsupported", Some(param.as_str())), "{model}: {}", err.message);
            assert!(err.message.contains(why), "{model}: {}", err.message);
            let out = validate_video(r.get(model).unwrap(), req(false)).expect(model);
            assert!(out.dropped.contains(&param), "{model}: {:?}", out.dropped);
            assert!(out.request.base.extra.as_ref().and_then(|e| e.get(key)).is_none(), "{model}: forwarded");
        }
    }
}
