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
}
