use std::path::{Path, PathBuf};
use thiserror::Error;

use crate::capabilities::schema::*;
use crate::types::{Model3dFormat, Model3dJointNaming, Model3dSkeleton, Model3dSymmetry, Model3dTopology, DEFAULT_MODEL3D_FORMAT};

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

    // 3D doc §2 rule 1: one task per model, from its own modality's list.
    if !m.task.allowed_for(m.media_type) {
        let allowed: Vec<&str> = ModelTask::ALL
            .iter()
            .filter(|t| t.allowed_for(m.media_type))
            .map(|t| t.as_str())
            .collect();
        return Err(bad(format!(
            "model '{}' task '{}' is not a {} task; {} tasks are {}",
            m.id, m.task.as_str(), m.media_type.as_str(), m.media_type.as_str(), allowed.join(" | ")
        )));
    }
    // The `*_to_3d` flags say how a 3D GENERATION is driven (its references
    // pick text, image or multiview). An operation takes a mesh instead, and
    // no other modality has a 3D mode at all.
    let c = &m.capabilities;
    for (flag, set) in [
        ("text_to_3d", c.text_to_3d),
        ("image_to_3d", c.image_to_3d),
        ("multiview_to_3d", c.multiview_to_3d),
    ] {
        if set && (m.media_type != MediaType::Model3d || m.task != ModelTask::Generate) {
            return Err(bad(format!(
                "model '{}' sets {} on a {} '{}' model; the *_to_3d flags belong to model3d \
                 models whose task is generate",
                m.id, flag, m.media_type.as_str(), m.task.as_str()
            )));
        }
    }

    // Every other 3D task works on an existing mesh and must say which ones
    // it takes (3D doc §2 rule 1, §3A).
    if m.media_type == MediaType::Model3d && m.task != ModelTask::Generate && !m.params.contains_key("source_model") {
        return Err(bad(format!(
            "model '{}' is a 3D '{}' operation but declares no params.source_model; \
             say which containers, sizes and face counts its vendor takes",
            m.id, m.task.as_str()
        )));
    }
    if let Some(ParamSpec::SourceModel(src)) = m.params.get("source_model") {
        if src.formats.is_empty() {
            return Err(bad(format!("model '{}' source_model declares no formats", m.id)));
        }
        let vocab: Vec<&str> = Model3dFormat::ALL.iter().map(|f| f.as_str()).collect();
        if let Some(f) = src.formats.iter().find(|f| !vocab.contains(&f.as_str())) {
            return Err(bad(format!(
                "model '{}' source_model format '{}' is not one of {:?}", m.id, f, vocab
            )));
        }
    }

    // 3D doc §2 rule 3: the axes litegen turns a vendor's mesh from.
    if let Some(o) = &m.native_orientation {
        if m.media_type != MediaType::Model3d {
            return Err(bad(format!("model '{}' declares native_orientation, which only model3d models have", m.id)));
        }
        use crate::mesh::normalize::Axis;
        if o.up == "source" || o.front == "source" {
            if o.up != o.front {
                return Err(bad(format!(
                    "model '{}' native_orientation keeps its source's axes only as a pair: up and front both `source`", m.id
                )));
            }
            if m.task == ModelTask::Generate {
                return Err(bad(format!(
                    "model '{}' native_orientation is `source`, but a generation model has no source to keep", m.id
                )));
            }
        }
        // Spelled exactly as the API reports it.
        let axis = |name: &str, v: &str| Axis::parse(v).filter(|a| a.as_str() == v).ok_or_else(|| bad(format!(
            "model '{}' native_orientation.{} '{}' is not one of +x, -x, +y, -y, +z, -z", m.id, name, v
        )));
        let keeps_source = o.up == "source";
        if !keeps_source {
        let (up, front) = (axis("up", &o.up)?, axis("front", &o.front)?);
        if up.as_str()[1..] == front.as_str()[1..] {
            return Err(bad(format!(
                "model '{}' native_orientation up '{}' and front '{}' are not at right angles", m.id, o.up, o.front
            )));
        }
        }
        if o.evidence.trim().is_empty() {
            return Err(bad(format!(
                "model '{}' native_orientation names no evidence; an orientation nobody has seen is a guess", m.id
            )));
        }
    }

    // 3D doc §4: callers send white-edits masks and litegen converts, so
    // every mask role names its vendor's own convention (P-R9). Only the
    // mask role has a convention or a native growth.
    if let Some(refs) = &m.ref_inputs {
        for (name, role) in &refs.roles {
            if name == "mask" {
                if role.convention.is_none() {
                    return Err(bad(format!(
                        "model '{}' mask role names no convention (white_edits | black_edits | transparent_edits): the vendor's own", m.id
                    )));
                }
            } else if role.convention.is_some() || role.native_grow {
                return Err(bad(format!(
                    "model '{}' role '{}' declares a mask convention or native_grow, which only the mask role has", m.id, name
                )));
            }
        }
    }
    let edits_part = m.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("mask")) || m.params.contains_key("region");
    if m.preserves_unmasked.is_some() && !edits_part {
        return Err(bad(format!(
            "model '{}' declares preserves_unmasked but takes neither a mask nor a region", m.id
        )));
    }
    if let Some(ParamSpec::Region(r)) = m.params.get("region") {
        if !m.ref_inputs.as_ref().is_some_and(|refs| refs.roles.contains_key("init")) {
            return Err(bad(format!("model '{}' takes a region but no init image to take it from", m.id)));
        }
        if r.max_aspect.is_some_and(|a| a < 1.0) {
            return Err(bad(format!("model '{}' region.max_aspect is below 1", m.id)));
        }
    }

    // 3D doc §3E: views come from one vocabulary, whether a multiview image
    // model makes them or a multiview-to-3D model takes them as `view-*`
    // roles; a multiview image model says which it makes, from an init image.
    let views: Vec<&str> = crate::types::ImageView::ALL.iter().map(|v| v.as_str()).collect();
    if let Some(spec) = m.params.get("views") {
        let ParamSpec::StringArray(s) = spec else {
            return Err(bad(format!("model '{}' views must be kind: string_array", m.id)));
        };
        if let Some(v) = s.enum_values.iter().chain(&s.default).find(|v| !views.contains(&v.as_str())) {
            return Err(bad(format!("model '{}' views value '{}' is not one of {:?}", m.id, v, views)));
        }
    }
    if m.task == ModelTask::Multiview
        && !(m.params.contains_key("views") && m.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("init")))
    {
        return Err(bad(format!("model '{}' is a multiview model: it declares the views it makes and takes an init image", m.id)));
    }
    if let Some(refs) = &m.ref_inputs {
        if let Some(r) = refs.roles.keys().filter_map(|r| r.strip_prefix("view-")).find(|v| !views.contains(v)) {
            return Err(bad(format!("model '{}' role 'view-{}' names no view of {:?}", m.id, r, views)));
        }
        // An ordered view list names views the model takes, each once.
        for (i, v) in refs.view_order.iter().enumerate() {
            if !refs.roles.contains_key(&format!("view-{v}")) || refs.view_order[..i].contains(v) {
                return Err(bad(format!(
                    "model '{}' view_order {:?}: each entry must be a view-<name> role the model declares, once",
                    m.id, refs.view_order
                )));
            }
        }
    }

    // 3D doc §5: timed keyframes are a video role, and say how their times
    // are placed.
    if let Some(refs) = &m.ref_inputs {
        for (name, role) in &refs.roles {
            let keyframe = name == "keyframe";
            if keyframe && m.media_type != MediaType::Video {
                return Err(bad(format!("model '{}' declares a keyframe role, which only video models take", m.id)));
            }
            if keyframe && role.timing.is_none() {
                return Err(bad(format!("model '{}' keyframe role must declare its timing (any | even)", m.id)));
            }
            if !keyframe && (role.timing.is_some() || role.time_step_s.is_some()) {
                return Err(bad(format!("model '{}' role '{name}': timing and time_step_s belong to the keyframe role", m.id)));
            }
            if role.time_step_s.is_some_and(|s| !(s > 0.0 && s.is_finite())) {
                return Err(bad(format!("model '{}' keyframe time_step_s must be a positive number of seconds", m.id)));
            }
        }
    }

    // 3D doc §3G, L14: each pricing rule has one effect, over facts a
    // request can have, in a unit its medium has.
    for rule in &m.pricing.variable_pricing {
        if rule.effects() != 1 {
            return Err(bad(format!("model '{}' pricing rule {:?} must name exactly one effect", m.id, rule)));
        }
        if rule.per_item_usd.is_some() != rule.per_item.is_some() {
            return Err(bad(format!("model '{}' pricing rule: per_item_usd and per_item go together", m.id)));
        }
        if let Some(list) = rule.per_item.as_deref().filter(|l| !super::pricing::DERIVED_FACTS.contains(l)) {
            return Err(bad(format!("model '{}' pricing rule counts '{list}', which is not a list litegen counts", m.id)));
        }
        if rule.per_second_usd.is_some() && m.media_type != MediaType::Video {
            return Err(bad(format!("model '{}' pricing rule: per_second_usd is for video models", m.id)));
        }
        if rule.per_megapixel_usd.is_some() && m.media_type != MediaType::Image {
            return Err(bad(format!("model '{}' pricing rule: per_megapixel_usd is for image models", m.id)));
        }
        if [rule.usd, rule.per_second_usd, rule.per_megapixel_usd, rule.per_item_usd, rule.multiply].iter().flatten().any(|v| *v < 0.0) {
            return Err(bad(format!("model '{}' pricing rule has a negative price or factor", m.id)));
        }
        for fact in rule.when.keys() {
            let known = m.params.contains_key(fact)
                || super::pricing::DERIVED_FACTS.contains(&fact.as_str())
                || fact.strip_prefix("extra.").is_some_and(|f| m.extra_allowlist.iter().any(|a| a == f));
            if !known {
                return Err(bad(format!(
                    "model '{}' pricing rule reads '{fact}', which is not a param, an allowlisted extra.<field> or one of {:?}",
                    m.id,
                    super::pricing::DERIVED_FACTS
                )));
            }
        }
    }

    // 3D doc §3C: the clips an animate model makes.
    if let Some(spec) = m.params.get("animations") {
        let ParamSpec::AnimationList(a) = spec else {
            return Err(bad(format!("model '{}' animations must be kind: animation_list", m.id)));
        };
        if m.media_type != MediaType::Model3d {
            return Err(bad(format!("model '{}' declares animations, which only model3d models have", m.id)));
        }
        if a.max_items == 0 {
            return Err(bad(format!("model '{}' animations.max_items must be at least 1", m.id)));
        }
        let vocab: Vec<&str> = crate::types::Model3dAnimationPreset::ALL.iter().map(|p| p.as_str()).collect();
        let mut seen: Vec<&str> = Vec::new();
        for p in &a.presets {
            if p.id.trim().is_empty() {
                return Err(bad(format!("model '{}' animations has a preset with no id", m.id)));
            }
            if seen.contains(&p.id.as_str()) {
                return Err(bad(format!("model '{}' animations lists preset '{}' twice", m.id, p.id)));
            }
            seen.push(&p.id);
            if let Some(c) = &p.canonical {
                if !vocab.contains(&c.as_str()) {
                    return Err(bad(format!(
                        "model '{}' animations preset '{}' is tagged '{}', which is not one of {:?}", m.id, p.id, c, vocab
                    )));
                }
            }
        }
        if let Some(t) = &a.text_to_motion {
            if !(t.min_duration_s > 0.0 && t.min_duration_s <= t.max_duration_s) {
                return Err(bad(format!(
                    "model '{}' animations.text_to_motion needs 0 < min_duration_s <= max_duration_s", m.id
                )));
            }
        }
        if a.presets.is_empty() && a.text_to_motion.is_none() {
            return Err(bad(format!("model '{}' animations offers neither presets nor text_to_motion", m.id)));
        }
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
    if let Some(ParamSpec::String(s)) = m.params.get("skeleton") {
        let vocab: Vec<&str> = Model3dSkeleton::ALL.iter().map(|t| t.as_str()).collect();
        in_vocab("skeleton", &s.enum_values, &vocab)?;
    }
    if let Some(ParamSpec::String(s)) = m.params.get("joint_naming") {
        let vocab: Vec<&str> = Model3dJointNaming::ALL.iter().map(|t| t.as_str()).collect();
        in_vocab("joint_naming", &s.enum_values, &vocab)?;
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
