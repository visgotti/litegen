#[cfg(test)]
mod tests {
    use crate::capabilities::*;

    #[test]
    fn round_trip_minimal_model() {
        let yaml = r#"
models:
  - id: x/model
    provider: x
    media_type: image
    display_name: Model
    pricing:
      base_cost_usd: 0.01
    capabilities:
      text_to_image: true
    prompt:
      required: true
      max_length: 100
"#;
        let parsed: ModelsFile = serde_yaml::from_str(yaml).unwrap();
        assert_eq!(parsed.models.len(), 1);
        assert_eq!(parsed.models[0].id, "x/model");
        assert!(parsed.models[0].capabilities.text_to_image);
    }

    #[test]
    fn round_trip_full_model() {
        let yaml = r#"
models:
  - id: x/full
    provider: x
    media_type: image
    display_name: Full
    pricing: { base_cost_usd: 0.02 }
    capabilities:
      text_to_image: true
      image_to_image: true
    prompt: { required: true, max_length: 4000 }
    params:
      seed:
        kind: seed
        min: 0
        max: 4294967294
      aspect_ratio:
        kind: aspect_ratio
        allowed: ["1:1", "16:9"]
        default: "1:1"
      size:
        kind: size
        mode: enum
        values:
          - [1024, 1024]
          - [1792, 1024]
    ref_inputs:
      max_total: 1
      default_role: init
      provider_format:
        form: multipart
        field_map:
          init: image
      roles:
        init:
          required: false
          min_count: 0
          max_count: 1
    extra_allowlist: [output_format]
    tags: [text-to-image]
"#;
        let parsed: ModelsFile = serde_yaml::from_str(yaml).unwrap();
        let m = &parsed.models[0];
        assert!(matches!(m.params["seed"], ParamSpec::Seed(ParamSpecSeed { min: 0, .. })));
        let ri = m.ref_inputs.as_ref().unwrap();
        assert_eq!(ri.max_total, 1);
        assert!(matches!(ri.provider_format, RefProviderFormat::Multipart(_)));
    }

    #[test]
    fn every_known_param_has_a_canonical_kind() {
        // The two lists move together: a KNOWN_PARAMS entry with no kind would
        // make the loader refuse every model that declares it.
        for p in KNOWN_PARAMS {
            assert!(!canonical_param_kinds(p).is_empty(), "'{p}' has no canonical kind");
        }
        assert!(canonical_param_kinds("not_a_param").is_empty());
    }

    #[test]
    fn param_spec_kind_matches_its_serialised_tag() {
        // `kind()` is what the loader compares against canonical_param_kinds,
        // and the serde tag is what the yaml and the API carry — they must be
        // the same word or the loader would judge a spec by the wrong name.
        let specs = [
            ParamSpec::Bool(Default::default()),
            ParamSpec::Int(Default::default()),
            ParamSpec::Float(Default::default()),
            ParamSpec::String(Default::default()),
            ParamSpec::StringArray(Default::default()),
            ParamSpec::Size(SizeSpec::Enum(SizeSpecEnum { values: vec![], label: None, description: None })),
            ParamSpec::AspectRatio(ParamSpecAspectRatio { allowed: vec![], default: None, label: None, description: None, frames_set_shape: None }),
            ParamSpec::Seed(ParamSpecSeed { min: 0, max: 1, label: None, description: None }),
        ];
        for spec in specs {
            let v = serde_json::to_value(&spec).unwrap();
            assert_eq!(v["kind"], spec.kind(), "{v}");
        }
    }

    /// E4a: where a result goes, where it is stored and who sees it are
    /// litegen's, in any spelling a vendor uses; other vendor fields are not.
    #[test]
    fn the_gateway_owns_where_results_go_and_who_sees_them() {
        for key in [
            "callback_url", "callbackUrl", "CallbackURL", "callback", "webhook_url", "webhook", "WebhookUrl",
            "status_callback", "fal_webhook", "notify_url", "storageUri", "storage_uri", "outputDataConfig",
            "isPublic", "is_public", "public", "Public",
        ] {
            assert!(gateway_owned_extra(key).is_some(), "{key} is litegen's");
        }
        assert!(gateway_owned_extra("callback_url").unwrap().contains("webhook"));
        assert!(gateway_owned_extra("isPublic").unwrap().contains("access"));
        for key in ["watermark", "seed", "model", "publication", "public_id", "prompt_optimizer", "safety_tolerance", "style_id"] {
            assert_eq!(gateway_owned_extra(key), None, "{key} may be allowlisted");
        }
    }

    #[test]
    fn derivable_values_is_served_only_when_present() {
        // Omitted rather than `[]` for every non-3D string array, and absent
        // from what yaml round-trips.
        let empty = serde_json::to_value(ParamSpecStringArray::default()).unwrap();
        assert!(empty.get("derivable_values").is_none(), "{empty}");
        let served = serde_json::to_value(ParamSpecStringArray {
            derivable_values: vec!["glb".into()],
            ..Default::default()
        }).unwrap();
        assert_eq!(served["derivable_values"], serde_json::json!(["glb"]));
    }

    /// Keeping nothing, litegen converts nothing, so a request is promised
    /// only what the model's one vendor job returns, asked as its adapter
    /// asks: each shipped 3D row that makes more than one container itself,
    /// request shape by request shape.
    #[test]
    fn keeping_nothing_promises_only_what_one_vendor_job_returns() {
        use crate::types::Model3dFormatSource;
        let mut dir = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        dir.pop();
        dir.push("models");
        let catalog = CapabilityRegistry::from_dir(&dir).unwrap();
        let glb_source = Some(Model3dFormatSource::Glb);
        let cases: &[(&str, &[&str], Option<Model3dFormatSource>, bool)] = &[
            // Tripo makes one container per job: its fbx when one is promised, else glb.
            ("tripo/rig", &["glb"], None, true),
            ("tripo/rig", &["fbx"], None, true),
            ("tripo/rig", &["glb", "fbx"], None, false),
            // Its retarget is always asked for glb.
            ("tripo/animate", &["glb"], None, true),
            ("tripo/animate", &["fbx"], None, false),
            // Meshy's rig and animation return a glb and an fbx at once; with
            // `format_source: glb` litegen takes the glb alone.
            ("meshy/rig", &["glb", "fbx"], None, true),
            ("meshy/rig", &["fbx"], None, true),
            ("meshy/rig", &["glb"], glb_source, true),
            ("meshy/rig", &["fbx"], glb_source, false),
            ("meshy/rig", &["glb", "fbx"], glb_source, false),
            ("meshy/animate", &["glb", "fbx"], None, true),
            ("meshy/animate", &["fbx"], glb_source, false),
            ("fal/meshy-rig", &["glb", "fbx"], None, true),
            ("fal/meshy-rig", &["fbx"], glb_source, false),
            // Rodin makes the one container it is asked for: its own fbx or
            // usdz, else glb, from which litegen would convert obj or stl.
            ("fal/hyper3d-rodin", &["glb"], None, true),
            ("fal/hyper3d-rodin", &["fbx"], None, true),
            ("fal/hyper3d-rodin", &["usdz"], None, true),
            ("fal/hyper3d-rodin", &["obj"], None, false),
            ("fal/hyper3d-rodin", &["stl"], None, false),
            ("fal/hyper3d-rodin", &["glb", "fbx"], None, false),
            ("fal/hyper3d-rodin", &["fbx", "usdz"], None, false),
            ("fal/hyper3d-rodin", &["fbx"], glb_source, false),
            // A glb-only vendor: its glb, and nothing it would have to convert.
            ("fal/trellis", &["glb"], None, true),
            ("fal/trellis", &["obj"], None, false),
        ];
        let wrong: Vec<String> = cases
            .iter()
            .filter_map(|&(id, formats, source, kept_nothing_of)| {
                let m = catalog.get(id).unwrap_or_else(|| panic!("{id} is not in the catalog"));
                let requested: Vec<String> = formats.iter().map(|f| f.to_string()).collect();
                let refusal = m.keep_nothing_refusal(&requested, source);
                (refusal.is_none() != kept_nothing_of).then(|| format!("{id} {formats:?} {source:?}: {refusal:?}"))
            })
            .collect();
        assert!(wrong.is_empty(), "{wrong:#?}");
    }
}
