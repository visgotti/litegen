use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::capabilities::schema::*;
use crate::types::{Model3dFormat, Model3dSymmetry, Model3dTopology, DEFAULT_MODEL3D_FORMAT};

#[derive(Debug, Error)]
pub enum LoadError {
    #[error("io error reading {path}: {source}")]
    Io { path: String, source: std::io::Error },

    #[error("yaml parse error in {path}: {message}")]
    Parse { path: String, message: String },

    #[error("validation error in {path}: {message}")]
    Validate { path: String, message: String },

    #[error("aggregate: {0:#?}")]
    Aggregate(Vec<LoadError>),
}

pub(crate) fn parse_file(path: &str, yaml: &str) -> Result<Vec<ModelSchema>, LoadError> {
    let file: ModelsFile = serde_yaml::from_str(yaml).map_err(|e| LoadError::Parse {
        path: path.to_string(),
        message: e.to_string(),
    })?;
    for m in &file.models {
        validate_model(path, m)?;
    }
    Ok(file.models)
}

pub(crate) fn validate_model(path: &str, m: &ModelSchema) -> Result<(), LoadError> {
    let bad = |msg: String| LoadError::Validate { path: path.to_string(), message: msg };

    // id format: "<provider>/<rest>"
    let (prefix, rest) = m.id.split_once('/').ok_or_else(|| bad(format!(
        "model id '{}' must be in form 'provider/name'", m.id
    )))?;
    if rest.is_empty() {
        return Err(bad(format!("model id '{}' must have a name after '/'", m.id)));
    }
    if prefix != m.provider {
        return Err(bad(format!(
            "model id '{}' provider prefix '{}' does not match field provider '{}'",
            m.id, prefix, m.provider
        )));
    }

    if m.pricing.base_cost_usd < 0.0 {
        return Err(bad(format!("model '{}' has negative base_cost_usd", m.id)));
    }

    // `max_length: 0` is how a model says it takes no prompt; requiring one as
    // well would make every request unsatisfiable.
    if m.prompt.max_length == Some(0) && m.prompt.required {
        return Err(bad(format!(
            "model '{}' prompt is required but max_length is 0 (which means no prompt)", m.id
        )));
    }

    for (k, spec) in &m.params {
        if !KNOWN_PARAMS.contains(&k.as_str()) {
            return Err(bad(format!(
                "model '{}' has unknown param key '{}'; known: {:?}",
                m.id, k, KNOWN_PARAMS
            )));
        }
        // A wrong kind is worse than an unknown key: it loads, is advertised
        // through `GET /v1/models`, and is then refused by the validator as
        // unsupported on every request that uses it.
        let kinds = canonical_param_kinds(k);
        if !kinds.contains(&spec.kind()) {
            return Err(bad(format!(
                "model '{}' declares '{}' as kind: {}; it must be kind: {}",
                m.id, k, spec.kind(), kinds.join(" | ")
            )));
        }
    }

    if let Some(url) = &m.docs_url {
        let ok = reqwest::Url::parse(url)
            .is_ok_and(|u| u.scheme() == "https" && u.host_str().is_some_and(|h| !h.is_empty()));
        if !ok {
            return Err(bad(format!(
                "model '{}' docs_url '{}' must be an absolute https:// URL", m.id, url
            )));
        }
    }

    // The closed 3D vocabularies. Validation compares case-insensitively, but
    // the catalog is what `GET /v1/models/{id}` serves, so it must use the
    // canonical spelling — and a typo here ("gbl") would otherwise advertise a
    // container no observer of completion could ever find.
    let in_vocab = |param: &str, values: &[String], vocab: &[&str]| -> Result<(), LoadError> {
        match values.iter().find(|v| !vocab.contains(&v.as_str())) {
            Some(v) => Err(bad(format!(
                "model '{}' {} value '{}' is not one of {:?}", m.id, param, v, vocab
            ))),
            None => Ok(()),
        }
    };
    let formats: Vec<&str> = Model3dFormat::ALL.iter().map(|f| f.as_str()).collect();
    if let Some(ParamSpec::StringArray(s)) = m.params.get("output_formats") {
        in_vocab("output_formats", &s.enum_values, &formats)?;
        in_vocab("output_formats default", &s.default, &formats)?;
        // Computed by the server from what the vendor emits. A catalog able to
        // set it could promise conversions litegen cannot perform.
        if !s.derivable_values.is_empty() {
            return Err(bad(format!(
                "model '{}' sets output_formats.derivable_values, which the server computes; remove it",
                m.id
            )));
        }
    }
    if let Some(ParamSpec::String(s)) = m.params.get("output_format") {
        in_vocab("output_format", &s.enum_values, &formats)?;
    }
    if let Some(ParamSpec::String(s)) = m.params.get("topology") {
        let vocab: Vec<&str> = Model3dTopology::ALL.iter().map(|t| t.as_str()).collect();
        in_vocab("topology", &s.enum_values, &vocab)?;
    }
    if let Some(ParamSpec::String(s)) = m.params.get("symmetry") {
        let vocab: Vec<&str> = Model3dSymmetry::ALL.iter().map(|t| t.as_str()).collect();
        in_vocab("symmetry", &s.enum_values, &vocab)?;
    }

    // `output_formats` sanity.
    //
    // These are the checks that keep a 3D model from advertising a container it
    // cannot emit — the failure mode this param exists to close. A wrong-kinded
    // spec is rejected outright rather than silently ignored, because
    // `ModelSchema::output_formats_spec` reads a mismatch as "no spec", which
    // would quietly disable format validation for the model.
    if let Some(spec) = m.params.get("output_formats") {
        let ParamSpec::StringArray(s) = spec else {
            return Err(bad(format!(
                "model '{}' output_formats must be kind: string_array", m.id
            )));
        };
        if s.enum_values.is_empty() {
            return Err(bad(format!(
                "model '{}' output_formats declares no enum_values", m.id
            )));
        }
        if !s.enum_values.iter().any(|v| v == DEFAULT_MODEL3D_FORMAT) {
            return Err(bad(format!(
                "model '{}' output_formats must include '{}' — it is the only container \
                 every 3D vendor can emit, and the one callers get by default",
                m.id, DEFAULT_MODEL3D_FORMAT
            )));
        }
        if let Some(max) = s.max_items {
            if max == 0 {
                return Err(bad(format!("model '{}' output_formats max_items is 0", m.id)));
            }
            if max > s.enum_values.len() {
                return Err(bad(format!(
                    "model '{}' output_formats max_items {} exceeds the {} declared enum_values",
                    m.id, max, s.enum_values.len()
                )));
            }
            if s.default.len() > max {
                return Err(bad(format!(
                    "model '{}' output_formats default has {} entries but max_items is {}",
                    m.id, s.default.len(), max
                )));
            }
        }
        for d in &s.default {
            if !s.enum_values.contains(d) {
                return Err(bad(format!(
                    "model '{}' output_formats default '{}' is not in enum_values", m.id, d
                )));
            }
        }
    }

    // The two spellings mean the same thing; carrying both leaves which one
    // wins to `output_formats_spec` rather than to the reader.
    if m.params.contains_key("output_formats") && m.params.contains_key("output_format") {
        return Err(bad(format!(
            "model '{}' declares both output_formats and the superseded output_format; \
             keep only output_formats", m.id
        )));
    }

    // SizeSpec::Freeform sanity
    if let Some(ParamSpec::Size(SizeSpec::Freeform(f))) = m.params.get("size") {
        if f.min_width > f.max_width {
            return Err(bad(format!(
                "model '{}' size: min_width {} > max_width {}", m.id, f.min_width, f.max_width
            )));
        }
        if f.min_height > f.max_height {
            return Err(bad(format!(
                "model '{}' size: min_height {} > max_height {}", m.id, f.min_height, f.max_height
            )));
        }
    }

    // RefInputSpec consistency
    if let Some(ri) = &m.ref_inputs {
        if let RefProviderFormat::Multipart(mp) = &ri.provider_format {
            for k in mp.field_map.keys() {
                if !ri.roles.contains_key(k) {
                    return Err(bad(format!(
                        "model '{}' ref_inputs.field_map role '{}' is not declared in roles",
                        m.id, k
                    )));
                }
            }
        }
        if let Some(default) = &ri.default_role {
            if !ri.roles.contains_key(default) {
                return Err(bad(format!(
                    "model '{}' ref_inputs.default_role '{}' is not declared in roles",
                    m.id, default
                )));
            }
        }
    }
    Ok(())
}

pub fn discover_model_files(dir: &Path) -> Result<Vec<PathBuf>, LoadError> {
    let entries = std::fs::read_dir(dir).map_err(|e| LoadError::Io {
        path: dir.display().to_string(),
        source: e,
    })?;
    let mut out = Vec::new();
    for entry in entries {
        let entry = entry.map_err(|e| LoadError::Io { path: dir.display().to_string(), source: e })?;
        let p = entry.path();
        if p.extension().and_then(|e| e.to_str()) == Some("yaml") {
            out.push(p);
        }
    }
    out.sort();
    Ok(out)
}
