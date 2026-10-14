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
            ParamSpec::AspectRatio(ParamSpecAspectRatio { allowed: vec![], default: None, label: None, description: None }),
            ParamSpec::Seed(ParamSpecSeed { min: 0, max: 1, label: None, description: None }),
        ];
        for spec in specs {
            let v = serde_json::to_value(&spec).unwrap();
            assert_eq!(v["kind"], spec.kind(), "{v}");
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
}
