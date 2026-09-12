use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::capabilities::schema::*;
use crate::types::DEFAULT_MODEL3D_FORMAT;

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

fn validate_model(path: &str, m: &ModelSchema) -> Result<(), LoadError> {
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

    for k in m.params.keys() {
        if !KNOWN_PARAMS.contains(&k.as_str()) {
            return Err(bad(format!(
                "model '{}' has unknown param key '{}'; known: {:?}",
                m.id, k, KNOWN_PARAMS
            )));
        }
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
