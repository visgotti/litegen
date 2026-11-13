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

    // PixelGenie L7: a segmentation model says which prompts it takes, only
    // it takes prompts, and it makes no images.
    let segments = m.media_type == MediaType::Image && m.task == ModelTask::Segment;
    match (segments, m.params.get("prompts")) {
        (true, None) => {
            return Err(bad(format!(
                "model '{}' is a segmentation model but declares no params.prompts: say which prompt types it takes", m.id
            )));
        }
        (false, Some(_)) => {
            return Err(bad(format!("model '{}' declares prompts, which only image models whose task is segment take", m.id)));
        }
        (true, Some(ParamSpec::SegmentationPrompts(p))) => {
            if p.types.is_empty() {
                return Err(bad(format!("model '{}' prompts.types is empty", m.id)));
            }
            if p.max_items == 0 || p.max_masks == 0 {
                return Err(bad(format!("model '{}' prompts.max_items and prompts.max_masks must be at least 1", m.id)));
            }
        }
        _ => {}
    }
    if segments {
        if let Some((flag, _)) = [("text_to_image", c.text_to_image), ("image_to_image", c.image_to_image), ("inpainting", c.inpainting)]
            .into_iter()
            .find(|(_, set)| *set)
        {
            return Err(bad(format!("model '{}' sets {} but a segmentation model makes no images", m.id, flag)));
        }
    }

    // 3D doc §5 (P2): every video task but generate works on a source clip,
    // and only those take one; a retake takes a window, and an extend says
    // what it returns.
    if m.media_type == MediaType::Video {
        let source = match m.params.get("source_video") {
            Some(ParamSpec::SourceVideo(s)) => Some(s),
            _ => None,
        };
        match (m.task, source) {
            (ModelTask::Generate, Some(_)) => {
                return Err(bad(format!("model '{}' takes source_video, which only video operations take", m.id)));
            }
            (ModelTask::Generate, None) => {}
            (task, None) => {
                return Err(bad(format!("model '{}' is a video '{}' operation but declares no params.source_video", m.id, task.as_str())));
            }
            (task, Some(s)) => {
                if s.formats.is_empty() {
                    return Err(bad(format!("model '{}' source_video declares no formats", m.id)));
                }
                if task == ModelTask::Extend && s.extend_output.is_none() {
                    return Err(bad(format!("model '{}' is an extend model: say its extend_output (full | added)", m.id)));
                }
                if task == ModelTask::Retake && !matches!(m.params.get("time_range"), Some(ParamSpec::TimeRange(_))) {
                    return Err(bad(format!("model '{}' is a retake model: declare the time_range it takes", m.id)));
                }
            }
        }
    }

    // 3D doc §4 (P2): a background swap redraws an init image's background
    // from a prompt.
    if m.task == ModelTask::BackgroundSwap {
        let init_required = m.ref_inputs.as_ref().and_then(|r| r.roles.get("init")).is_some_and(|r| r.required);
        if !init_required || m.prompt.max_length == Some(0) {
            return Err(bad(format!("model '{}' is a background_swap model: it takes a required init image and a prompt", m.id)));
        }
    }

    // 3D doc §4 (P2): a layout composes boxes on a new, text-to-image canvas.
    if let Some(ParamSpec::Layout(l)) = m.params.get("layout") {
        if m.media_type != MediaType::Image || !c.text_to_image {
            return Err(bad(format!("model '{}' takes a layout but makes no image from text", m.id)));
        }
        if l.kinds.is_empty() || l.max_items == 0 {
            return Err(bad(format!("model '{}' layout names no box kinds or no boxes", m.id)));
        }
    }

    // 3D doc §4 (P2): expand grows an init image's canvas.
    if matches!(m.params.get("expand"), Some(ParamSpec::Expand(_)))
        && !(m.media_type == MediaType::Image && m.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("init")))
    {
        return Err(bad(format!("model '{}' takes expand but no init image to grow", m.id)));
    }

    // 3D doc §3F: a texture model lists the map sizes it paints at, so the
    // listing's `texture_resolutions` names every size the validator takes.
    if let Some(ParamSpec::Int(r)) = m.params.get("texture_resolution") {
        if r.values.is_empty() {
            return Err(bad(format!("model '{}' texture_resolution lists no values: name the sizes it paints at", m.id)));
        }
    }

    // PixelGenie L8: a background-removal model takes one init image and no
    // prompt, does nothing else, and answers in a container with alpha.
    if c.background_removal {
        let refuse = |why: String| Err(bad(format!("model '{}' sets background_removal: {why}", m.id)));
        if m.media_type != MediaType::Image || m.task != ModelTask::Generate {
            return refuse("only image models whose task is generate remove backgrounds".into());
        }
        if let Some((flag, _)) = [("text_to_image", c.text_to_image), ("image_to_image", c.image_to_image), ("inpainting", c.inpainting)]
            .into_iter()
            .find(|(_, set)| *set)
        {
            return refuse(format!("it sets {flag} too; a matting model only removes backgrounds"));
        }
        if m.prompt.max_length != Some(0) {
            return refuse("it takes no prompt (prompt.max_length: 0)".into());
        }
        let init = m.ref_inputs.as_ref().and_then(|r| r.roles.get("init").map(|role| (r.max_total, role)));
        if !matches!(init, Some((1, role)) if role.required && role.max_count == 1) {
            return refuse("it takes exactly one required init image and no other reference".into());
        }
        if let Some(ParamSpec::String(f)) = m.params.get("output_format") {
            if let Some(lossy) = f.enum_values.iter().find(|v| !matches!(v.as_str(), "png" | "webp")) {
                return refuse(format!("output_format '{lossy}' has no alpha channel; offer png or webp"));
            }
        }
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

    // PixelGenie L10: a camera from the closed vocabulary, and a duration
    // list inside the duration range.
    if let Some(ParamSpec::String(s)) = m.params.get("camera") {
        if let Some(v) = s.enum_values.iter().find(|v| !["fixed", "auto"].contains(&v.as_str())) {
            return Err(bad(format!("model '{}' camera value '{v}' is not one of [\"fixed\", \"auto\"]", m.id)));
        }
    }
    // E4b: a typed extra is an allowlisted key that is no param of the row,
    // of a plain JSON kind, whose default is a value it takes.
    for (key, spec) in &m.extra_params {
        if !m.extra_allowlist.contains(key) {
            return Err(bad(format!("model '{}' extra_params '{key}' is not on the allowlist; add it to extra_allowlist", m.id)));
        }
        if !ParamSpec::EXTRA_KINDS.contains(&spec.kind()) {
            return Err(bad(format!(
                "model '{}' extra_params '{key}' is kind `{}`; an extra is one of {}",
                m.id, spec.kind(), ParamSpec::EXTRA_KINDS.join(", ")
            )));
        }
        if m.params.contains_key(key) {
            return Err(bad(format!("model '{}' extra_params '{key}' is one of the row's params; type it there only", m.id)));
        }
        if let Some(why) = super::pricing::param_default(spec).and_then(|d| spec.extra_misfit(&d)) {
            return Err(bad(format!("model '{}' extra_params '{key}' default is not a value it takes: {why}", m.id)));
        }
    }

    // N4: a `pattern` is a regular expression, on a param or a typed extra.
    for (key, spec) in m.params.iter().chain(&m.extra_params) {
        if let ParamSpec::String(ParamSpecString { pattern: Some(p), .. }) = spec {
            if let Err(e) = regex::Regex::new(p) {
                return Err(bad(format!("model '{}' {key} pattern {p:?} is not a regular expression: {e}", m.id)));
            }
        }
    }

    // Any number param's list lies inside its own range, a typed extra's too.
    let extras: Vec<(String, &ParamSpec)> = m.extra_params.iter().map(|(k, s)| (format!("extra.{k}"), s)).collect();
    let mut numbers: Vec<(&String, &ParamSpec)> = m.params.iter().chain(extras.iter().map(|(k, s)| (k, *s))).collect();
    numbers.sort_by(|a, b| a.0.cmp(b.0));
    for (key, spec) in numbers {
        let listed: Vec<f64> = match spec {
            ParamSpec::Float(f) => f.values.iter().copied().filter(|v| f.min.is_some_and(|m| *v < m) || f.max.is_some_and(|m| *v > m)).collect(),
            ParamSpec::Int(i) => i.values.iter().copied().filter(|v| i.min.is_some_and(|m| *v < m) || i.max.is_some_and(|m| *v > m)).map(|v| v as f64).collect(),
            _ => Vec::new(),
        };
        if let Some(v) = listed.first() {
            return Err(bad(format!("model '{}' {key} value {v} is outside its min/max", m.id)));
        }
    }

    // L14: lifecycle dates are calendar days, a model retires no earlier
    // than it was deprecated, and it is not its own replacement.
    let day = |field: &str, v: &Option<String>| -> Result<Option<chrono::NaiveDate>, LoadError> {
        v.as_deref()
            .map(|d| chrono::NaiveDate::parse_from_str(d, "%Y-%m-%d").map_err(|_| bad(format!("model '{}' {field} '{d}' is not YYYY-MM-DD", m.id))))
            .transpose()
    };
    let (deprecated, retires) = (day("deprecated_at", &m.deprecated_at)?, day("retires_at", &m.retires_at)?);
    if let (Some(d), Some(r)) = (deprecated, retires) {
        if r < d {
            return Err(bad(format!("model '{}' retires_at {r} is before its deprecated_at {d}", m.id)));
        }
    }
    if m.replaced_by.as_deref() == Some(m.id.as_str()) {
        return Err(bad(format!("model '{}' cannot be its own replaced_by", m.id)));
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
        if m.media_type != MediaType::Image {
            if let Some(field) = [("per_megapixel_usd", rule.per_megapixel_usd), ("multiply_per_megapixel_above", rule.multiply_per_megapixel_above)]
                .into_iter()
                .find_map(|(name, v)| v.map(|_| name))
            {
                return Err(bad(format!("model '{}' pricing rule: {field} is for image models", m.id)));
            }
        }
        if [rule.usd, rule.per_second_usd, rule.per_megapixel_usd, rule.per_item_usd, rule.multiply].iter().flatten().any(|v| *v < 0.0)
            || rule.multiply_per_megapixel_above.is_some_and(|v| v <= 0.0)
        {
            return Err(bad(format!("model '{}' pricing rule has a negative price or factor", m.id)));
        }
        for (fact, want) in &rule.when {
            if let serde_json::Value::Object(range) = want {
                let bounds = super::pricing::RANGE_BOUNDS;
                if range.is_empty() || range.iter().any(|(b, n)| !bounds.contains(&b.as_str()) || !n.is_number()) {
                    return Err(bad(format!(
                        "model '{}' pricing rule's range for '{fact}' must give a number for one or more of {bounds:?}",
                        m.id
                    )));
                }
            }
            let known = m.params.contains_key(fact)
                || super::pricing::DERIVED_FACTS.contains(&fact.as_str())
                || fact.strip_prefix("extra.").is_some_and(|f| m.extra_allowlist.iter().any(|a| a == f))
                || fact
                    .strip_prefix("references.")
                    .is_some_and(|role| m.ref_inputs.as_ref().is_some_and(|ri| ri.roles.contains_key(role)));
            if !known {
                return Err(bad(format!(
                    "model '{}' pricing rule reads '{fact}', which is not a param, an allowlisted extra.<field> or one of {:?}",
                    m.id,
                    super::pricing::DERIVED_FACTS
                )));
            }
            // E4b, N3: a price compares an extra exactly as sent (`true` is
            // not `"true"`), so the extra is typed and the rule's value is one
            // a request could send: `"*"`, a range for a number, or a value
            // of its type.
            if let Some(extra) = fact.strip_prefix("extra.") {
                let Some(spec) = m.extra_params.get(extra) else {
                    return Err(bad(format!(
                        "model '{}' pricing rule reads '{fact}', an untyped extra: type it in extra_params, since a price compares the value exactly",
                        m.id
                    )));
                };
                let fits = match want {
                    serde_json::Value::String(s) if s == "*" => Ok(()),
                    serde_json::Value::Object(_) if matches!(spec, ParamSpec::Int(_) | ParamSpec::Float(_)) => Ok(()),
                    serde_json::Value::Object(_) => Err("a range is for a number".to_string()),
                    v => spec.extra_misfit(v).map_or(Ok(()), |why| Err(why.to_string())),
                };
                if let Err(why) = fits {
                    return Err(bad(format!(
                        "model '{}' pricing rule's `when` value {want} for '{fact}' is not one its {} extra takes: {why}",
                        m.id,
                        spec.kind()
                    )));
                }
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

    // E4a: where a result goes, where it is stored and who sees it are
    // litegen's, whatever a vendor documents.
    if let Some((key, why)) = m.extra_allowlist.iter().find_map(|k| gateway_owned_extra(k).map(|why| (k, why))) {
        return Err(bad(format!(
            "model '{}' extra_allowlist names '{key}', a field litegen owns ({why}); remove it", m.id
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
        // No whitespace: `Url::parse` would percent-encode it, so a docs_url
        // that swallowed a folded `description:` line would load.
        let ok = !url.chars().any(char::is_whitespace)
            && reqwest::Url::parse(url)
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
    match (m.media_type, m.params.get("output_format"), m.params.get("background")) {
        (MediaType::Model3d, Some(ParamSpec::String(s)), _) => in_vocab("output_format", &s.enum_values, &formats)?,
        // PixelGenie L9: an image's container and background, from closed
        // vocabularies the validator and every adapter share.
        (MediaType::Image, out, bg) => {
            let containers: Vec<&str> = crate::types::ImageOutputFormat::ALL.iter().map(|f| f.as_str()).collect();
            match out {
                Some(ParamSpec::String(s)) => {
                    in_vocab("output_format", &s.enum_values, &containers)?;
                    in_vocab("output_format default", &s.default.iter().cloned().collect::<Vec<_>>(), &s.enum_values.iter().map(String::as_str).collect::<Vec<_>>())?;
                }
                Some(_) => return Err(bad(format!("model '{}' output_format must be kind: string", m.id))),
                None => {}
            }
            match bg {
                Some(ParamSpec::String(s)) => in_vocab("background", &s.enum_values, &["transparent", "opaque"])?,
                Some(_) => return Err(bad(format!("model '{}' background must be kind: string", m.id))),
                None => {}
            }
        }
        _ => {}
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
