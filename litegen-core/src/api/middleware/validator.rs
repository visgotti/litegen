// litegen-core/src/api/middleware/validator.rs
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::capabilities::*;
use crate::types::*;

// Catalog logic, now in capabilities (engine boundary, D5); kept at this path
// for its callers.
pub use crate::capabilities::resolve_duration_seconds;

#[derive(Debug, Clone, PartialEq)]
pub struct ValidationError {
    pub code: String,
    pub message: String,
    pub param: Option<String>,
}

impl ValidationError {
    fn new(code: &str, msg: impl Into<String>, param: Option<&str>) -> Self {
        Self { code: code.into(), message: msg.into(), param: param.map(str::to_string) }
    }
}

#[derive(Debug)]
pub struct ImageValidationOutput {
    pub request: ImageGenerationRequest,
    pub dropped: Vec<String>,
}

#[derive(Debug)]
pub struct VideoValidationOutput {
    pub request: VideoGenerationRequest,
    pub dropped: Vec<String>,
}

/// Segmentation models answer at `/v1/images/segmentations` and nowhere
/// else; every other image model at `/v1/images/generations` (PixelGenie
/// L7). `served` is `Segment` for the segmentation endpoint and `Generate`
/// for the generations one.
pub fn require_task(schema: &ModelSchema, served: ModelTask) -> Result<(), ValidationError> {
    let segments = schema.task == ModelTask::Segment;
    if segments == (served == ModelTask::Segment) {
        return Ok(());
    }
    let (what, endpoint) = if segments {
        ("a segmentation model", "/v1/images/segmentations")
    } else {
        ("not a segmentation model", "/v1/images/generations")
    };
    Err(ValidationError::new(
        "model_task_mismatch",
        format!("model '{}' is {what}; use {endpoint}", schema.id),
        Some("model"),
    ))
}

/// A segmentation request's prompts against the model (PixelGenie L7): its
/// prompt types and count, one text prompt alone, labels 0 or 1, boxes with
/// an area, and something to select.
pub fn validate_segmentation(schema: &ModelSchema, req: &ImageSegmentationRequest) -> Result<(), ValidationError> {
    let bad = |code: &str, msg: String| Err(ValidationError::new(code, msg, Some("prompts")));
    let Some(ParamSpec::SegmentationPrompts(spec)) = schema.params.get("prompts") else {
        return bad("capability_unsupported", format!("model '{}' takes no segmentation prompts", schema.id));
    };
    check_metadata(&req.metadata)?;
    if req.prompts.is_empty() {
        return bad("param_required", "prompts is empty: send a point, box or text prompt".into());
    }
    if let Some(p) = req.prompts.iter().find(|p| !spec.types.contains(&p.kind())) {
        let takes: Vec<&str> = spec.types.iter().map(|t| t.as_str()).collect();
        return bad(
            "capability_unsupported",
            format!("model '{}' takes {} prompts, not {}", schema.id, takes.join(", "), p.kind().as_str()),
        );
    }
    if req.prompts.len() > spec.max_items {
        return bad("param_out_of_range", format!("{} prompts; model '{}' takes at most {}", req.prompts.len(), schema.id, spec.max_items));
    }
    let texts = req.prompts.iter().filter(|p| matches!(p, SegmentationPrompt::Text { .. })).count();
    if texts > 1 {
        return bad("param_out_of_range", format!("{texts} text prompts; send one per request"));
    }
    if texts == 1 && req.prompts.len() > 1 {
        return bad(
            "param_conflict",
            "a text prompt selects every instance of what it names and cannot be mixed with point or box prompts".into(),
        );
    }
    for p in &req.prompts {
        match p {
            SegmentationPrompt::Point { label, .. } if *label > 1 => {
                return bad("param_out_of_range", format!("point label {label}: 1 selects the spot, 0 leaves it out"));
            }
            SegmentationPrompt::Box { w, h, .. } if *w == 0 || *h == 0 => {
                return bad("param_out_of_range", format!("a {w}×{h} box selects nothing"));
            }
            SegmentationPrompt::Text { text } if text.trim().is_empty() => {
                return bad("param_required", "a text prompt names nothing".into());
            }
            _ => {}
        }
    }
    let selects = req.prompts.iter().any(|p| !matches!(p, SegmentationPrompt::Point { label: 0, .. }));
    if !selects {
        return bad("param_conflict", "every point is label 0, which only leaves spots out: add a label-1 point or a box".into());
    }
    Ok(())
}

/// Every point and box lies inside a `width` × `height` image.
pub fn check_segmentation_bounds(prompts: &[SegmentationPrompt], width: u32, height: u32) -> Result<(), ValidationError> {
    for p in prompts {
        let inside = match *p {
            SegmentationPrompt::Point { x, y, .. } => x < width && y < height,
            SegmentationPrompt::Box { x, y, w, h } => {
                u64::from(x) + u64::from(w) <= u64::from(width) && u64::from(y) + u64::from(h) <= u64::from(height)
            }
            SegmentationPrompt::Text { .. } => true,
        };
        if !inside {
            return Err(ValidationError::new(
                "prompt_out_of_bounds",
                format!("{p:?} is not inside the {width}×{height} image"),
                Some("prompts"),
            ));
        }
    }
    Ok(())
}

#[tracing::instrument(skip(schema, req), fields(model = %schema.id))]
pub fn validate_image(
    schema: &ModelSchema,
    mut req: ImageGenerationRequest,
) -> Result<ImageValidationOutput, ValidationError> {
    let mut dropped = Vec::new();
    let strict = req.base.strict;

    check_prompt(schema, &mut req.base.prompt, strict, &mut dropped)?;
    check_metadata(&req.base.metadata)?;
    check_n(&req.base, "image")?;
    check_image_edit(schema, &mut req, strict, &mut dropped)?;
    check_views(schema, &mut req, strict, &mut dropped)?;
    check_expand(schema, &mut req, strict, &mut dropped)?;
    check_layout(schema, &mut req, strict, &mut dropped)?;
    check_mask_prompt(schema, &mut req, strict, &mut dropped)?;
    check_preserve(&req)?;
    check_pixel_art(schema, &mut req, strict, &mut dropped)?;

    macro_rules! check_param {
        ($field:expr, $key:literal, $variant:pat => $body:block) => {
            if $field.is_some() {
                match schema.params.get($key) {
                    Some($variant) => $body,
                    Some(_) => {
                        return Err(ValidationError::new(
                            "param_unsupported",
                            format!("Parameter '{}' has an incompatible kind for model '{}'.", $key, schema.id),
                            Some($key),
                        ));
                    }
                    None => {
                        if strict {
                            return Err(ValidationError::new(
                                "param_unsupported",
                                format!("Parameter '{}' is not supported by model '{}'.", $key, schema.id),
                                Some($key),
                            ));
                        } else {
                            dropped.push($key.to_string());
                            $field = None;
                        }
                    }
                }
            }
        };
    }

    check_param!(req.base.seed, "seed", ParamSpec::Seed(s) => {
        let v = req.base.seed.unwrap();
        if v < s.min || v > s.max {
            return Err(ValidationError::new(
                "param_out_of_range",
                format!("seed {} outside [{}, {}]", v, s.min, s.max),
                Some("seed"),
            ));
        }
    });

    check_param!(req.base.negative_prompt, "negative_prompt", ParamSpec::String(s) => {
        if let Some(m) = s.max_length {
            let v = req.base.negative_prompt.as_deref().unwrap();
            if v.len() > m {
                return Err(ValidationError::new(
                    "param_too_long",
                    format!("negative_prompt length {} exceeds {}", v.len(), m),
                    Some("negative_prompt"),
                ));
            }
        }
        check_pattern(req.base.negative_prompt.as_deref().unwrap(), s, "negative_prompt")?;
    });

    check_param!(req.steps, "steps", ParamSpec::Int(s) => {
        check_int(req.steps.unwrap() as i64, s, "steps", &schema.id)?;
    });

    check_param!(req.guidance_scale, "guidance_scale", ParamSpec::Float(s) => {
        check_float(req.guidance_scale.unwrap(), s, "guidance_scale", &schema.id)?;
    });

    check_param!(req.strength, "strength", ParamSpec::Float(s) => {
        check_float(req.strength.unwrap(), s, "strength", &schema.id)?;
    });

    check_param!(req.quality, "quality", ParamSpec::String(s) => {
        let v = req.quality.as_deref().unwrap();
        check_string(v, s, "quality")?;
    });

    // The output tier, on one scale for every vendor (2026-11-03).
    check_param!(req.resolution, "resolution", ParamSpec::String(s) => {
        let v = req.resolution.as_deref().unwrap();
        check_string(v, s, "resolution")?;
    });

    check_param!(req.style, "style", ParamSpec::String(s) => {
        let v = req.style.as_deref().unwrap();
        check_string(v, s, "style")?;
    });

    // PixelGenie L9: the container and background a caller asks for.
    check_param!(req.output_format, "output_format", ParamSpec::String(s) => {
        let v = req.output_format.as_deref().unwrap();
        check_string(v, s, "output_format")?;
    });
    check_param!(req.background, "background", ParamSpec::String(s) => {
        let v = req.background.as_deref().unwrap();
        check_string(v, s, "background")?;
    });
    if req.background.as_deref() == Some("transparent") {
        let format = req.output_format.clone().or_else(|| match schema.params.get("output_format") {
            Some(ParamSpec::String(s)) => s.default.clone(),
            _ => None,
        });
        if format.as_deref() == Some("jpeg") {
            return Err(ValidationError::new(
                "param_conflict",
                "a transparent background needs output_format png or webp; JPEG has no alpha channel",
                Some("background"),
            ));
        }
    }

    check_param!(req.aspect_ratio, "aspect_ratio", ParamSpec::AspectRatio(s) => {
        let v = req.aspect_ratio.as_deref().unwrap();
        if !s.allowed.iter().any(|a| a == v) {
            return Err(ValidationError::new(
                "param_enum_mismatch",
                format!("aspect_ratio '{}' not in {:?}", v, s.allowed),
                Some("aspect_ratio"),
            ));
        }
    });

    check_param!(req.size, "size", ParamSpec::Size(spec) => {
        let v = req.size.as_deref().unwrap();
        check_size(v, spec, "size")?;
    });

    check_refs(schema, &mut req.base.reference_images, strict, &mut dropped)?;
    check_image_request(schema, &req)?;
    check_extra(schema, &mut req.base.extra, strict, &mut dropped)?;

    Ok(ImageValidationOutput { request: req, dropped })
}

#[tracing::instrument(skip(schema, req), fields(model = %schema.id))]
pub fn validate_video(
    schema: &ModelSchema,
    mut req: VideoGenerationRequest,
) -> Result<VideoValidationOutput, ValidationError> {
    let mut dropped = Vec::new();
    let strict = req.base.strict;

    check_prompt(schema, &mut req.base.prompt, strict, &mut dropped)?;
    check_metadata(&req.base.metadata)?;
    check_n(&req.base, "video")?;

    // seed, negative_prompt — same as image
    if req.base.seed.is_some() {
        match schema.params.get("seed") {
            Some(ParamSpec::Seed(s)) => {
                let v = req.base.seed.unwrap();
                if v < s.min || v > s.max {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("seed {} outside [{}, {}]", v, s.min, s.max),
                        Some("seed"),
                    ));
                }
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("seed not supported by '{}'", schema.id),
                        Some("seed"),
                    ));
                }
                dropped.push("seed".into());
                req.base.seed = None;
            }
        }
    }

    if req.base.negative_prompt.is_some() {
        match schema.params.get("negative_prompt") {
            Some(ParamSpec::String(s)) => {
                if let Some(m) = s.max_length {
                    let v = req.base.negative_prompt.as_deref().unwrap();
                    if v.len() > m {
                        return Err(ValidationError::new(
                            "param_too_long",
                            format!("negative_prompt length {} > {}", v.len(), m),
                            Some("negative_prompt"),
                        ));
                    }
                }
                check_pattern(req.base.negative_prompt.as_deref().unwrap(), s, "negative_prompt")?;
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("negative_prompt not supported by '{}'", schema.id),
                        Some("negative_prompt"),
                    ));
                }
                dropped.push("negative_prompt".into());
                req.base.negative_prompt = None;
            }
        }
    }

    // duration_seconds is special — float typically; an Int spec is also
    // accepted. Every value sent is checked (E1): 0 and below once meant
    // "unset" and skipped these checks, so they were priced at 0 s while the
    // vendor made, and billed, its default clip.
    if let Some(requested) = req.duration_seconds {
        match schema.params.get("duration_seconds") {
            Some(ParamSpec::Float(s)) => {
                let v = requested;
                check_clip_length(requested, v)?;
                if s.min.map(|m| v < m).unwrap_or(false) || s.max.map(|m| v > m).unwrap_or(false) {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("duration_seconds {} out of range", v),
                        Some("duration_seconds"),
                    ));
                }
                // A vendor that takes a list, not a range (PixelGenie L10).
                if !s.values.is_empty() && !s.values.iter().any(|a| (a - v).abs() < 1e-9) {
                    return Err(ValidationError::new(
                        "param_enum_mismatch",
                        format!("'{}' takes duration_seconds {:?}; got {v}", schema.id, s.values),
                        Some("duration_seconds"),
                    ));
                }
            }
            Some(ParamSpec::Int(s)) => {
                // The whole seconds the vendor makes are the length checked.
                check_clip_length(requested, requested.round())?;
                let v = requested.round() as i64;
                if s.min.map(|m| v < m).unwrap_or(false) || s.max.map(|m| v > m).unwrap_or(false) {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("duration_seconds {} out of range", v),
                        Some("duration_seconds"),
                    ));
                }
                if !s.values.is_empty() && !s.values.contains(&v) {
                    return Err(ValidationError::new(
                        "param_enum_mismatch",
                        format!("'{}' takes duration_seconds {:?}; got {v}", schema.id, s.values),
                        Some("duration_seconds"),
                    ));
                }
                // The vendor makes whole seconds: the price, and every
                // adapter, see the length it makes (C5).
                req.duration_seconds = Some(v as f64);
            }
            // A model whose listing has no duration takes none (E6): its clip
            // length is the vendor's, or its keyframes'. Refused when strict,
            // dropped and named otherwise, like every other param it lacks.
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("'{}' takes no duration_seconds: its clip length is the vendor's (or its keyframes')", schema.id),
                        Some("duration_seconds"),
                    ));
                }
                dropped.push("duration_seconds".into());
                req.duration_seconds = None;
            }
        }
    }

    if req.aspect_ratio.is_some() {
        match schema.params.get("aspect_ratio") {
            Some(ParamSpec::AspectRatio(s)) => {
                let v = req.aspect_ratio.as_deref().unwrap();
                if !s.allowed.iter().any(|a| a == v) {
                    return Err(ValidationError::new(
                        "param_enum_mismatch",
                        format!("aspect_ratio '{}' not in {:?}", v, s.allowed),
                        Some("aspect_ratio"),
                    ));
                }
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("aspect_ratio not supported by '{}'", schema.id),
                        Some("aspect_ratio"),
                    ));
                }
                dropped.push("aspect_ratio".into());
                req.aspect_ratio = None;
            }
        }
    }

    if req.fps.is_some() {
        match schema.params.get("fps") {
            Some(ParamSpec::Int(s)) => check_int(req.fps.unwrap() as i64, s, "fps", &schema.id)?,
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("fps not supported by '{}'", schema.id),
                        Some("fps"),
                    ));
                }
                dropped.push("fps".into());
                req.fps = None;
            }
        }
    }

    if req.resolution.is_some() {
        match schema.params.get("resolution") {
            Some(ParamSpec::String(s)) => {
                let v = req.resolution.as_deref().unwrap();
                check_string(v, s, "resolution")?;
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("resolution not supported by '{}'", schema.id),
                        Some("resolution"),
                    ));
                }
                dropped.push("resolution".into());
                req.resolution = None;
            }
        }
    }

    check_video_controls(schema, &mut req, strict, &mut dropped)?;
    check_video_operation(schema, &mut req, strict, &mut dropped)?;
    check_refs(schema, &mut req.base.reference_images, strict, &mut dropped)?;
    check_frame_shape(schema, &mut req, strict, &mut dropped)?;
    check_video_request(schema, &req)?;
    check_keyframes(schema, &mut req)?;
    check_extra(schema, &mut req.base.extra, strict, &mut dropped)?;

    Ok(VideoValidationOutput { request: req, dropped })
}

/// A ratio the frame image overrides (E5): on a row whose `aspect_ratio`
/// declares `frames_set_shape`, the vendor takes no ratio with a
/// `first_frame`, `last_frame` or `init` image, so the ratio is refused when
/// strict and dropped and named otherwise, where the adapter used to leave it
/// out unreported. `without_references` lifts the rule when a `reference`
/// image rides along. Runs after `check_refs`, so a role-less image counts
/// under its `default_role`.
fn check_frame_shape(
    schema: &ModelSchema,
    req: &mut VideoGenerationRequest,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let Some(ParamSpec::AspectRatio(spec)) = schema.params.get("aspect_ratio") else { return Ok(()) };
    let Some(shape) = spec.frames_set_shape else { return Ok(()) };
    if req.aspect_ratio.is_none() {
        return Ok(());
    }
    let has = |roles: &[&str]| req.base.reference_images.iter().any(|r| r.role.as_deref().is_some_and(|role| roles.contains(&role)));
    let framed = has(&["first_frame", "last_frame", "init"]);
    let lifted = shape == FramesSetShape::WithoutReferences && has(&["reference"]);
    if !framed || lifted {
        return Ok(());
    }
    if strict {
        let unless = if shape == FramesSetShape::Always { "" } else { " unless a reference image goes with it" };
        return Err(ValidationError::new(
            "param_unsupported",
            format!("'{}' takes the frame image's shape: aspect_ratio applies only without a first_frame, last_frame or init image{unless}", schema.id),
            Some("aspect_ratio"),
        ));
    }
    dropped.push("aspect_ratio".into());
    req.aspect_ratio = None;
    Ok(())
}

/// A clip lasts more than 0 s (E1), in either mode: a bad value, not a param
/// the model lacks. `length` is what the vendor would make of `requested`
/// (its whole seconds on an int spec), so 0.4 s on a whole-second model is
/// refused too. A model without a `min` is held to this as well.
fn check_clip_length(requested: f64, length: f64) -> Result<(), ValidationError> {
    if length.is_finite() && length > 0.0 {
        return Ok(());
    }
    let made = if length == requested { String::new() } else { format!(" (a clip of {length:?} s)") };
    Err(ValidationError::new(
        "param_out_of_range",
        format!("duration_seconds {requested:?}{made}: a clip lasts more than 0 s"),
        Some("duration_seconds"),
    ))
}

#[derive(Debug)]
pub struct Model3dValidationOutput {
    pub request: Model3dGenerationRequest,
    pub dropped: Vec<String>,
}

#[tracing::instrument(skip(schema, req), fields(model = %schema.id))]
pub fn validate_model3d(
    schema: &ModelSchema,
    mut req: Model3dGenerationRequest,
) -> Result<Model3dValidationOutput, ValidationError> {
    let mut dropped = Vec::new();
    let strict = req.base.strict;

    check_prompt(schema, &mut req.base.prompt, strict, &mut dropped)?;
    check_metadata(&req.base.metadata)?;
    check_n(&req.base, "mesh")?;

    // seed / negative_prompt — identical treatment to image and video.
    if req.base.seed.is_some() {
        match schema.params.get("seed") {
            Some(ParamSpec::Seed(s)) => {
                let v = req.base.seed.unwrap();
                if v < s.min || v > s.max {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("seed {} outside [{}, {}]", v, s.min, s.max),
                        Some("seed"),
                    ));
                }
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("seed not supported by '{}'", schema.id),
                        Some("seed"),
                    ));
                }
                dropped.push("seed".into());
                req.base.seed = None;
            }
        }
    }

    if req.base.negative_prompt.is_some() {
        match schema.params.get("negative_prompt") {
            Some(ParamSpec::String(s)) => {
                let v = req.base.negative_prompt.as_deref().unwrap();
                check_string(v, s, "negative_prompt")?;
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("negative_prompt not supported by '{}'", schema.id),
                        Some("negative_prompt"),
                    ));
                }
                dropped.push("negative_prompt".into());
                req.base.negative_prompt = None;
            }
        }
    }

    // ─── 3D params ──────────────────────────────────────────────────────────
    //
    // Each follows the same shape: a declared spec validates the VALUE (errors
    // in both modes); an absent or mismatched spec means UNSUPPORTED, which
    // errors in strict mode and drops in lax mode. That split is what lets a
    // client send a superset of params without a capability table of its own.

    macro_rules! string_param {
        ($field:expr, $key:literal) => {
            if $field.is_some() {
                match schema.params.get($key) {
                    Some(ParamSpec::String(s)) => {
                        let v = $field.as_deref().unwrap();
                        check_string(v, s, $key)?;
                    }
                    Some(_) | None => {
                        if strict {
                            return Err(ValidationError::new(
                                "param_unsupported",
                                format!("{} not supported by '{}'", $key, schema.id),
                                Some($key),
                            ));
                        }
                        dropped.push($key.into());
                        $field = None;
                    }
                }
            }
        };
    }

    macro_rules! bool_param {
        ($field:expr, $key:literal) => {
            if $field.is_some() && !matches!(schema.params.get($key), Some(ParamSpec::Bool(_))) {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("{} not supported by '{}'", $key, schema.id),
                        Some($key),
                    ));
                }
                dropped.push($key.into());
                $field = None;
            }
        };
    }

    // `output_formats` — the one param whose validated value becomes a promise.
    //
    // Whatever survives here is enforced again at every completion observer, so
    // an unsatisfiable request has to die now, before the vendor is billed.
    // The superseded scalar folds in first so the two spellings cannot disagree.
    if req.output_formats.is_none() {
        req.output_formats = req.output_format.take().map(|f| vec![f]);
    } else {
        // Both sent. The plural is canonical; say so rather than silently
        // discarding one of them.
        if req.output_format.take().is_some() {
            dropped.push("output_format".into());
        }
    }

    if let Some(requested) = req.output_formats.as_mut() {
        // Bounded before anything walks the list: the vocabulary is ten
        // containers, and the de-duplication below is quadratic, so an
        // unbounded array (the 25 MiB body allows millions of entries) would pin
        // a runtime worker for hours before the enum check ever ran.
        const MAX_LISTED: usize = 64;
        if requested.len() > MAX_LISTED {
            return Err(ValidationError::new(
                "param_too_many",
                format!(
                    "output_formats lists {} entries; at most {} are accepted",
                    requested.len(),
                    MAX_LISTED
                ),
                Some("output_formats"),
            ));
        }
        // Case-fold and de-duplicate before checking. `["glb", "GLB"]` is one
        // container asked for twice, not a request for two.
        let mut seen: Vec<String> = Vec::new();
        for f in requested.iter() {
            let f = f.trim().to_ascii_lowercase();
            if !f.is_empty() && !seen.contains(&f) {
                seen.push(f);
            }
        }
        *requested = seen;

        if requested.is_empty() {
            return Err(ValidationError::new(
                "param_out_of_range",
                "output_formats must name at least one format",
                Some("output_formats"),
            ));
        }

        match schema.output_formats_spec() {
            Some(spec) => {
                // Deliverable, not native: a container litegen can derive
                // locally from one the vendor does emit is just as real to the
                // caller. Without this, `output_formats` on a GLB-only model —
                // which is most of the aggregator-hosted catalogue — could only
                // ever be `["glb"]`.
                let deliverable =
                    crate::proxy::model3d_convert::deliverable_formats(&spec.enum_values);
                for f in requested.iter() {
                    if !deliverable.iter().any(|v| v.eq_ignore_ascii_case(f)) {
                        return Err(ValidationError::new(
                            "param_enum_mismatch",
                            format!(
                                "output_format '{}' not supported by '{}'; supported: {:?}",
                                f, schema.id, deliverable
                            ),
                            Some("output_formats"),
                        ));
                    }
                }
                // `max_items` bounds what the VENDOR must produce, not what the
                // caller may ask for: every derivable format comes out of one
                // readable source, so a whole set of them costs one vendor job.
                // That is what lets a one-format-per-job vendor serve four
                // containers while still refusing two that must both come from
                // it.
                if let Some(max) = spec.max_items {
                    let needed = crate::proxy::model3d_convert::vendor_jobs_needed(requested);
                    if needed > max {
                        return Err(ValidationError::new(
                            "param_too_many",
                            format!(
                                "'{}' emits at most {} format(s) per generation; this request \
                                 needs {} from the provider",
                                schema.id, max, needed
                            ),
                            Some("output_formats"),
                        ));
                    }
                }
            }
            None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("output_formats not supported by '{}'", schema.id),
                        Some("output_formats"),
                    ));
                }
                dropped.push("output_formats".into());
                req.output_formats = None;
            }
        }
    }

    string_param!(req.symmetry, "symmetry");
    string_param!(req.topology, "topology");
    string_param!(req.skeleton, "skeleton");
    string_param!(req.joint_naming, "joint_naming");
    // 3D doc §3B: every scheme but `vendor` names humanoid bones.
    if req.joint_naming.as_deref().is_some_and(|n| n != "vendor") {
        let skeleton = req.skeleton.clone().or_else(|| match schema.params.get("skeleton") {
            Some(ParamSpec::String(s)) => s.default.clone(),
            _ => None,
        });
        if let Some(other) = skeleton.filter(|s| s != "humanoid") {
            return Err(ValidationError::new(
                "param_conflict",
                format!("joint_naming {:?} names humanoid bones; a {other} rig keeps its vendor's names", req.joint_naming.as_deref().unwrap_or_default()),
                Some("joint_naming"),
            ));
        }
    }
    if let Some(v) = req.height_m {
        match schema.params.get("height_m") {
            // A model's own range; with no ceiling of its own, litegen's
            // placement ceiling still applies (types `height_m`: "at most
            // 10,000 m, or within a model's own range").
            Some(ParamSpec::Float(s)) => {
                if !v.is_finite() || v <= 0.0 || s.min.is_some_and(|m| v < m) || s.max.unwrap_or(MAX_PLACEMENT_HEIGHT_M) < v {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("height_m {} out of range", v),
                        Some("height_m"),
                    ));
                }
                check_float(v, s, "height_m", &schema.id)?;
            }
            // 3D doc §3D: placement — litegen scales any mesh it delivers.
            Some(_) | None => {
                if !v.is_finite() || v <= 0.0 || v > MAX_PLACEMENT_HEIGHT_M {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("height_m {v}: litegen scales a mesh to a height above 0 and at most {MAX_PLACEMENT_HEIGHT_M} m"),
                        Some("height_m"),
                    ));
                }
            }
        }
    }
    if let Some(v) = req.fps {
        match schema.params.get("fps") {
            Some(ParamSpec::Int(s)) => check_int(v as i64, s, "fps", &schema.id)?,
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("fps not supported by '{}'", schema.id),
                        Some("fps"),
                    ));
                }
                dropped.push("fps".into());
                req.fps = None;
            }
        }
    }
    // An animate model makes the clips the request names; with none named
    // there is nothing for it to make (Meshy's adapter refuses at submit).
    // (An explicitly empty list is `check_animations`' out-of-range case.)
    if schema.task == crate::capabilities::ModelTask::Animate && req.animations.is_none() {
        return Err(ValidationError::new(
            "param_required",
            format!("'{}' animates a rig: name at least one clip in `animations`", schema.id),
            Some("animations"),
        ));
    }
    if req.animations.is_some() {
        match schema.params.get("animations") {
            Some(ParamSpec::AnimationList(spec)) => check_animations(req.animations.as_deref().unwrap_or_default(), spec, &schema.id)?,
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("animations not supported by '{}'", schema.id),
                        Some("animations"),
                    ));
                }
                dropped.push("animations".into());
                req.animations = None;
            }
        }
    }
    bool_param!(req.texture, "texture");
    bool_param!(req.pbr, "pbr");
    bool_param!(req.rig, "rig");
    bool_param!(req.keep_uvs, "keep_uvs");
    // 3D doc §3F: a map size the model paints at, one of its listed values.
    if let Some(v) = req.texture_resolution {
        match schema.params.get("texture_resolution") {
            Some(ParamSpec::Int(s)) => {
                let v = i64::from(v);
                if !s.values.is_empty() && !s.values.contains(&v) {
                    return Err(ValidationError::new(
                        "param_enum_mismatch",
                        format!("texture_resolution {v}: '{}' paints at {:?}", schema.id, s.values),
                        Some("texture_resolution"),
                    ));
                }
                if s.min.is_some_and(|m| v < m) || s.max.is_some_and(|m| v > m) {
                    return Err(ValidationError::new("param_out_of_range", format!("texture_resolution {v} out of range"), Some("texture_resolution")));
                }
            }
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("texture_resolution not supported by '{}'", schema.id),
                        Some("texture_resolution"),
                    ));
                }
                dropped.push("texture_resolution".into());
                req.texture_resolution = None;
            }
        }
    }

    if req.target_polycount.is_some() {
        match schema.params.get("target_polycount") {
            Some(ParamSpec::Int(s)) => check_int(req.target_polycount.unwrap() as i64, s, "target_polycount", &schema.id)?,
            Some(_) | None => {
                if strict {
                    return Err(ValidationError::new(
                        "param_unsupported",
                        format!("target_polycount not supported by '{}'", schema.id),
                        Some("target_polycount"),
                    ));
                }
                dropped.push("target_polycount".into());
                req.target_polycount = None;
            }
        }
    }

    // `source_model` (3D doc §3A): the mesh an operation works on. Its shape
    // is checked here; reading it, and refusing what the vendor would refuse,
    // needs storage and the network and happens before the billed submit.
    match (req.source_model.as_ref(), schema.params.get("source_model")) {
        (Some(src), Some(ParamSpec::SourceModel(_))) => {
            match (src.kind, src.format.as_deref()) {
                (SourceKind::Url | SourceKind::Base64, None) => {
                    return Err(ValidationError::new(
                        "source_format_required",
                        "a url or base64 source_model must name its format",
                        Some("source_model"),
                    ));
                }
                (_, Some(f)) if Model3dFormat::parse(f).is_none() => {
                    return Err(ValidationError::new(
                        "source_format_unsupported",
                        format!(
                            "source_model format '{}' is not a mesh container litegen reads; one of {:?}",
                            f,
                            Model3dFormat::ALL.iter().map(|f| f.as_str()).collect::<Vec<_>>()
                        ),
                        Some("source_model"),
                    ));
                }
                _ => {}
            }
            if src.value.trim().is_empty() {
                return Err(ValidationError::new("source_model_required", "source_model.value is empty", Some("source_model")));
            }
        }
        (Some(_), _) => {
            if strict {
                return Err(ValidationError::new(
                    "param_unsupported",
                    format!("source_model not supported by '{}'", schema.id),
                    Some("source_model"),
                ));
            }
            dropped.push("source_model".into());
            req.source_model = None;
        }
        (None, Some(ParamSpec::SourceModel(_))) => {
            return Err(ValidationError::new(
                "source_model_required",
                format!("'{}' is a {} operation and needs source_model: the mesh to work on", schema.id, schema.task.as_str()),
                Some("source_model"),
            ));
        }
        (None, _) => {}
    }

    check_refs(schema, &mut req.base.reference_images, strict, &mut dropped)?;
    check_multiview_refs(schema, &req.base.reference_images)?;

    // 3D doc §3F: a texture model paints from a prompt, a style image, or both.
    if schema.task == ModelTask::Texture
        && req.base.prompt.trim().is_empty()
        && !req.base.reference_images.iter().any(|r| r.role.as_deref() == Some("style"))
    {
        return Err(ValidationError::new(
            "param_required",
            format!("'{}' paints from a prompt, a style reference image, or both; send one", schema.id),
            Some("prompt"),
        ));
    }

    // The mode is inferred from the images that survived `check_refs`: none
    // means text-to-3D. That needs a model with a text arm, and a prompt to
    // generate from — a model whose prompt is optional only because its IMAGE
    // arm can do without one (Rodin) would otherwise submit an empty request
    // for the vendor to reject, after the quota is reserved. A model whose
    // `init` role is required never gets here without an image.
    if req.base.reference_images.is_empty() && schema.task == ModelTask::Generate {
        if !schema.capabilities.text_to_3d {
            return Err(ValidationError::new(
                "capability_unsupported",
                format!("'{}' has no text-to-3D mode; send a reference image", schema.id),
                Some("reference_images"),
            ));
        }
        if req.base.prompt.trim().is_empty() {
            return Err(ValidationError::new(
                "prompt_required",
                "text-to-3D needs a prompt; send one, or a reference image for image-to-3D",
                Some("prompt"),
            ));
        }
    }

    check_extra(schema, &mut req.base.extra, strict, &mut dropped)?;

    Ok(Model3dValidationOutput { request: req, dropped })
}

/// Check the prompt against the model's `PromptSpec`.
///
/// `max_length: 0` declares a model that takes NO prompt — an image-only
/// endpoint whose vendor schema has no prompt field (fal's TRELLIS and
/// Hunyuan3D v2). Sending one there is an unsupported param like any other:
/// refused in strict mode, dropped and reported in lax mode. Never silently
/// ignored, which is what forwarding nothing while answering 200 would be.
///
/// Lengths count characters, not bytes: vendor limits come from JSON Schema
/// `maxLength`, which counts code points, so a byte count would refuse a
/// non-ASCII prompt the vendor itself accepts.
fn check_prompt(
    schema: &ModelSchema,
    prompt: &mut String,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let spec = &schema.prompt;
    let empty = prompt.trim().is_empty();
    if spec.max_length == Some(0) {
        if !empty {
            if strict {
                return Err(ValidationError::new(
                    "param_unsupported",
                    format!("'{}' takes no prompt; omit it, or send strict: false to have it dropped", schema.id),
                    Some("prompt"),
                ));
            }
            dropped.push("prompt".into());
            prompt.clear();
        }
        return Ok(());
    }
    if empty {
        if spec.required {
            return Err(ValidationError::new(
                "prompt_required",
                "prompt is required",
                Some("prompt"),
            ));
        }
        return Ok(());
    }
    let len = prompt.chars().count();
    if let Some(min) = spec.min_length {
        if len < min {
            return Err(ValidationError::new(
                "prompt_too_short",
                format!("prompt length {} < {}", len, min),
                Some("prompt"),
            ));
        }
    }
    if let Some(max) = spec.max_length {
        if len > max {
            return Err(ValidationError::new(
                "prompt_too_long",
                format!("prompt length {} > {}", len, max),
                Some("prompt"),
            ));
        }
    }
    Ok(())
}

/// The most a request's `metadata` may hold, as JSON (L17).
pub const MAX_METADATA_BYTES: usize = 1024;

/// Whether `key` can name a `metadata` entry: what `metadata.<key>` in a
/// `/v1/logs` or `/v1/stats` query, and a JSON path, both carry unquoted.
pub fn is_metadata_key(key: &str) -> bool {
    (1..=64).contains(&key.len()) && key.bytes().all(|b| b.is_ascii_alphanumeric() || matches!(b, b'_' | b'.' | b'-'))
}

/// `metadata` is the caller's own string map, kept on the row (L17): any
/// other shape is refused rather than stored, since nothing could filter on it.
fn check_metadata(metadata: &Option<serde_json::Value>) -> Result<(), ValidationError> {
    let Some(value) = metadata.as_ref().filter(|v| !v.is_null()) else { return Ok(()) };
    let bad = |msg: String| ValidationError::new("param_out_of_range", msg, Some("metadata"));
    let map = value.as_object().ok_or_else(|| bad("metadata must be an object of string values".into()))?;
    for (k, v) in map {
        if !is_metadata_key(k) {
            return Err(bad(format!("metadata key {k:?} must be 1-64 letters, digits, '_', '.' or '-'")));
        }
        if !v.is_string() {
            return Err(bad(format!("metadata.{k} must be a string")));
        }
    }
    let bytes = value.to_string().len();
    if bytes > MAX_METADATA_BYTES {
        return Err(ValidationError::new(
            "param_too_long",
            format!("metadata is {bytes} bytes as JSON > {MAX_METADATA_BYTES}"),
            Some("metadata"),
        ));
    }
    Ok(())
}

/// A partial edit's shape (3D doc §4): `region` where the model takes one,
/// `mask_grow_px` where it takes a mask or a region, and an init image for
/// either to apply to. Whether the region fits the init image needs the image
/// itself, and is checked before the billed call (`proxy::image_edit`).
fn check_image_edit(
    schema: &ModelSchema,
    req: &mut ImageGenerationRequest,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    if req.region.is_some() {
        match schema.params.get("region") {
            Some(ParamSpec::Region(_)) => {}
            _ => {
                if strict {
                    return Err(ValidationError::new("param_unsupported", format!("region not supported by '{}'", schema.id), Some("region")));
                }
                dropped.push("region".into());
                req.region = None;
            }
        }
    }
    let takes_mask = schema.ref_inputs.as_ref().is_some_and(|r| r.roles.contains_key("mask"));
    if let Some(grow) = req.mask_grow_px {
        if !(takes_mask || schema.params.contains_key("region")) {
            if strict {
                return Err(ValidationError::new("param_unsupported", format!("mask_grow_px not supported by '{}'", schema.id), Some("mask_grow_px")));
            }
            dropped.push("mask_grow_px".into());
            req.mask_grow_px = None;
        } else if grow > MAX_MASK_GROW_PX {
            return Err(ValidationError::new(
                "param_out_of_range",
                format!("mask_grow_px {grow} is above {MAX_MASK_GROW_PX}"),
                Some("mask_grow_px"),
            ));
        }
    }
    let role = |r: &ReferenceImage, name: &str| r.role.as_deref() == Some(name);
    let has_mask = req.base.reference_images.iter().any(|r| role(r, "mask"));
    let has_init = req.base.reference_images.iter().any(|r| role(r, "init") || r.role.is_none());
    if (req.region.is_some() || has_mask) && !has_init {
        return Err(ValidationError::new(
            "init_required",
            "a mask or region edits an init image; send one with role init",
            Some("reference_images"),
        ));
    }
    Ok(())
}

/// `expand` (3D doc §4, P2): a model that grows canvases, an init image to
/// grow, something added, within the vendor's per-side limit, symmetric for
/// a vendor that only centres, and neither a mask nor a region beside it.
fn check_expand(
    schema: &ModelSchema,
    req: &mut ImageGenerationRequest,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let Some(e) = req.expand else { return Ok(()) };
    let Some(ParamSpec::Expand(spec)) = schema.params.get("expand") else {
        if strict {
            return Err(ValidationError::new("param_unsupported", format!("expand not supported by '{}'", schema.id), Some("expand")));
        }
        dropped.push("expand".into());
        req.expand = None;
        return Ok(());
    };
    let refs = &req.base.reference_images;
    if !refs.iter().any(|r| r.role.as_deref() == Some("init") || r.role.is_none()) {
        return Err(ValidationError::new("init_required", "expand grows an init image's canvas; send one with role init", Some("reference_images")));
    }
    if refs.iter().any(|r| r.role.as_deref() == Some("mask")) || req.region.is_some() {
        return Err(ValidationError::new("param_conflict", "expand grows the canvas; it cannot be combined with a mask or a region", Some("expand")));
    }
    let sides = [e.left, e.top, e.right, e.bottom];
    if sides.iter().all(|s| *s == 0) {
        return Err(ValidationError::new("param_out_of_range", "expand adds nothing: give at least one side some pixels", Some("expand")));
    }
    if let Some(max) = spec.max_px.filter(|max| sides.iter().any(|s| s > max)) {
        return Err(ValidationError::new("param_out_of_range", format!("'{}' adds at most {max} px to a side", schema.id), Some("expand")));
    }
    if spec.centered_only && (e.left != e.right || e.top != e.bottom) {
        return Err(ValidationError::new(
            "expand_unsupported",
            format!("'{}' only centres the source: left must equal right, and top bottom", schema.id),
            Some("expand"),
        ));
    }
    Ok(())
}

/// `layout` (3D doc §4, P2): boxes on a new canvas — the model's kinds, at
/// most its count, each inside the canvas with an area, an object described
/// and text given (one word where the vendor takes one), and no init image.
fn check_layout(
    schema: &ModelSchema,
    req: &mut ImageGenerationRequest,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let Some(items) = req.layout.as_ref() else { return Ok(()) };
    let Some(ParamSpec::Layout(spec)) = schema.params.get("layout") else {
        if strict {
            return Err(ValidationError::new("param_unsupported", format!("layout not supported by '{}'", schema.id), Some("layout")));
        }
        dropped.push("layout".into());
        req.layout = None;
        return Ok(());
    };
    let bad = |code: &str, msg: String| Err(ValidationError::new(code, msg, Some("layout")));
    if items.is_empty() || items.len() > spec.max_items {
        return bad("param_out_of_range", format!("{} layout boxes; '{}' takes 1 to {}", items.len(), schema.id, spec.max_items));
    }
    if req.base.reference_images.iter().any(|r| r.role.as_deref() == Some("init") || r.role.is_none()) {
        return bad("param_conflict", "layout composes a new image; it takes no init image".into());
    }
    for (i, item) in items.iter().enumerate() {
        let b = item.bounds;
        let inside = [b.x, b.y, b.w, b.h].iter().all(|v| v.is_finite())
            && b.x >= 0.0 && b.y >= 0.0 && b.w > 0.0 && b.h > 0.0
            && b.x + b.w <= 1.0 + 1e-9 && b.y + b.h <= 1.0 + 1e-9;
        if !inside {
            return bad("param_out_of_range", format!("layout box {i} is not inside the canvas (fractions 0 to 1, with an area)"));
        }
        if !spec.kinds.contains(&item.kind) {
            let takes: Vec<&str> = spec.kinds.iter().map(|k| k.as_str()).collect();
            return bad("capability_unsupported", format!("'{}' lays out {} boxes, not {}", schema.id, takes.join(" and "), item.kind.as_str()));
        }
        let given = |v: &Option<String>| v.as_deref().is_some_and(|s| !s.trim().is_empty());
        match item.kind {
            LayoutKind::Object if !given(&item.prompt) => return bad("param_required", format!("layout box {i} is an object: describe it in prompt")),
            LayoutKind::Text if !given(&item.text) => return bad("param_required", format!("layout box {i} is text: give its text")),
            LayoutKind::Text if spec.single_word_text && item.text.as_deref().is_some_and(|t| t.trim().contains(char::is_whitespace)) => {
                return bad("param_out_of_range", format!("'{}' places one word per text box; box {i} has several", schema.id));
            }
            _ => {}
        }
    }
    Ok(())
}

/// `mask_prompt` (3D doc §4, P2): a model that finds the area itself, an init
/// image to edit, and neither a mask nor a region beside it.
fn check_mask_prompt(
    schema: &ModelSchema,
    req: &mut ImageGenerationRequest,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let Some(text) = req.mask_prompt.as_deref() else { return Ok(()) };
    let Some(ParamSpec::String(spec)) = schema.params.get("mask_prompt") else {
        if strict {
            return Err(ValidationError::new("param_unsupported", format!("mask_prompt not supported by '{}'", schema.id), Some("mask_prompt")));
        }
        dropped.push("mask_prompt".into());
        req.mask_prompt = None;
        return Ok(());
    };
    if text.trim().is_empty() {
        return Err(ValidationError::new("param_required", "mask_prompt names nothing", Some("mask_prompt")));
    }
    check_string(text, spec, "mask_prompt")?;
    let refs = &req.base.reference_images;
    if !refs.iter().any(|r| r.role.as_deref() == Some("init") || r.role.is_none()) {
        return Err(ValidationError::new("init_required", "mask_prompt names an area of an init image; send one with role init", Some("reference_images")));
    }
    if refs.iter().any(|r| r.role.as_deref() == Some("mask")) || req.region.is_some() {
        return Err(ValidationError::new("param_conflict", "mask_prompt names the area itself; send it without a mask or a region", Some("mask_prompt")));
    }
    Ok(())
}

/// `preserve_unmasked` and `feather_px` (3D doc §4, P2): a mask or region to
/// keep outside of, a lossless container, and a feather only with it.
fn check_preserve(req: &ImageGenerationRequest) -> Result<(), ValidationError> {
    let preserve = req.preserve_unmasked == Some(true);
    let conflict = |msg: &str, param: &str| Err(ValidationError::new("param_conflict", msg, Some(param)));
    if req.feather_px.is_some() && !preserve {
        return conflict("feather_px softens preserve_unmasked's edge; send it with preserve_unmasked: true", "feather_px");
    }
    if req.feather_px.is_some_and(|f| f > MAX_FEATHER_PX) {
        return Err(ValidationError::new("param_out_of_range", format!("feather_px is at most {MAX_FEATHER_PX}"), Some("feather_px")));
    }
    if !preserve {
        return Ok(());
    }
    if req.region.is_none() && !req.base.reference_images.iter().any(|r| r.role.as_deref() == Some("mask")) {
        return conflict("preserve_unmasked keeps the pixels outside a mask or region; send one", "preserve_unmasked");
    }
    if req.output_format.as_deref() == Some("jpeg") {
        return conflict("preserve_unmasked returns pixels byte-identical, which JPEG cannot; ask for png or webp", "output_format");
    }
    Ok(())
}

/// `palette` and `directions` (PixelGenie L16): `#rrggbb` colours, at most the
/// model's count; a directions count the model lists.
fn check_pixel_art(
    schema: &ModelSchema,
    req: &mut ImageGenerationRequest,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let undeclared = |name: &str| ValidationError::new("param_unsupported", format!("{name} not supported by '{}'", schema.id), Some(name));
    if let Some(colours) = req.palette.as_ref() {
        match schema.params.get("palette") {
            Some(ParamSpec::Palette(spec)) => {
                let hex = |c: &str| c.len() == 7 && c.starts_with('#') && c[1..].chars().all(|ch| ch.is_ascii_hexdigit());
                if colours.is_empty() {
                    return Err(ValidationError::new("param_out_of_range", "palette names no colours", Some("palette")));
                }
                if let Some(bad) = colours.iter().find(|c| !hex(c)) {
                    return Err(ValidationError::new("param_out_of_range", format!("palette colour '{bad}' is not #rrggbb"), Some("palette")));
                }
                if colours.len() > spec.max_colors {
                    return Err(ValidationError::new("param_too_many", format!("{} colours; '{}' keeps to at most {}", colours.len(), schema.id, spec.max_colors), Some("palette")));
                }
            }
            _ if strict => return Err(undeclared("palette")),
            _ => {
                dropped.push("palette".into());
                req.palette = None;
            }
        }
    }
    if let Some(n) = req.directions {
        match schema.params.get("directions") {
            Some(ParamSpec::Int(spec)) if spec.values.is_empty() || spec.values.contains(&i64::from(n)) => {}
            Some(ParamSpec::Int(spec)) => {
                return Err(ValidationError::new("param_enum_mismatch", format!("directions {n}: '{}' draws {:?}", schema.id, spec.values), Some("directions")));
            }
            _ if strict => return Err(undeclared("directions")),
            _ => {
                dropped.push("directions".into());
                req.directions = None;
            }
        }
    }
    Ok(())
}

/// The widest `feather_px`.
pub const MAX_FEATHER_PX: u32 = 256;

/// `views` (3D doc §3E): views the model makes, each once, at least one.
fn check_views(
    schema: &ModelSchema,
    req: &mut ImageGenerationRequest,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let Some(views) = req.views.as_ref() else { return Ok(()) };
    let Some(ParamSpec::StringArray(spec)) = schema.params.get("views") else {
        if strict {
            return Err(ValidationError::new("param_unsupported", format!("views not supported by '{}'", schema.id), Some("views")));
        }
        dropped.push("views".into());
        req.views = None;
        return Ok(());
    };
    if views.is_empty() {
        return Err(ValidationError::new("param_out_of_range", "views must name at least one view", Some("views")));
    }
    let mut seen: Vec<&str> = Vec::new();
    for v in views {
        if !spec.enum_values.iter().any(|e| e == v) {
            return Err(ValidationError::new(
                "param_enum_mismatch",
                format!("'{}' makes no view '{v}'; it makes {:?}", schema.id, spec.enum_values),
                Some("views"),
            ));
        }
        if seen.contains(&v.as_str()) {
            return Err(ValidationError::new("param_enum_mismatch", format!("view '{v}' is listed twice"), Some("views")));
        }
        seen.push(v);
    }
    if spec.max_items.is_some_and(|max| views.len() > max) {
        return Err(ValidationError::new("param_too_many", format!("'{}' makes at most {:?} views", schema.id, spec.max_items), Some("views")));
    }
    Ok(())
}

/// The tallest `height_m` litegen scales a mesh to where the model declares
/// no range of its own.
pub const MAX_PLACEMENT_HEIGHT_M: f64 = 10_000.0;

/// The most a mask may grow: Stability's own `grow_mask` ceiling.
pub const MAX_MASK_GROW_PX: u32 = 100;

/// A clip list against the model's `animation_list` spec (3D doc §3C): every
/// name unique and 1–64 of `[A-Za-z0-9_.-]`; each clip exactly one of a
/// preset the model plays (canonical or its own id) and a prompt (where it
/// makes motion from text, for a length within its range).
fn check_animations(
    clips: &[AnimationRequest],
    spec: &crate::capabilities::ParamSpecAnimationList,
    model: &str,
) -> Result<(), ValidationError> {
    let fail = |code: &str, msg: String| Err(ValidationError::new(code, msg, Some("animations")));
    if clips.is_empty() {
        return fail("param_out_of_range", "animations must name at least one clip".into());
    }
    if clips.len() > spec.max_items {
        return fail("param_too_many", format!("'{model}' makes at most {} clips per request; {} asked", spec.max_items, clips.len()));
    }
    let mut names: Vec<&str> = Vec::new();
    for c in clips {
        let well_formed = (1..=64).contains(&c.name.chars().count())
            && c.name.chars().all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '.' | '-'));
        if !well_formed {
            return fail("animation_name_invalid", format!("clip name '{}' must be 1–64 characters of A–Z, a–z, 0–9, _ . -", c.name));
        }
        if names.contains(&c.name.as_str()) {
            return fail("animation_name_invalid", format!("clip name '{}' is used twice; names must be unique", c.name));
        }
        names.push(&c.name);
        match (c.preset.as_deref(), c.prompt.as_deref()) {
            (Some(p), None) => {
                if c.duration_s.is_some() {
                    return fail("animation_clip_invalid", format!("clip '{}': duration_s is for prompt clips; a preset lasts as long as it lasts", c.name));
                }
                let known = spec.presets.iter().any(|s| s.id == p || s.canonical.as_deref() == Some(p));
                if !known {
                    let offered: Vec<&str> = spec
                        .presets
                        .iter()
                        .flat_map(|s| s.canonical.as_deref().into_iter().chain(std::iter::once(s.id.as_str())))
                        .collect();
                    return fail("param_enum_mismatch", format!("clip '{}': '{model}' has no preset '{p}'; it plays {offered:?}", c.name));
                }
            }
            (None, Some(_)) => {
                let Some(t) = &spec.text_to_motion else {
                    return fail("param_unsupported", format!("clip '{}': '{model}' makes no motion from a prompt", c.name));
                };
                if let Some(d) = c.duration_s {
                    if !(d.is_finite() && d >= t.min_duration_s && d <= t.max_duration_s) {
                        return fail("param_out_of_range", format!(
                            "clip '{}': duration_s {d} is outside {}–{} s", c.name, t.min_duration_s, t.max_duration_s
                        ));
                    }
                }
            }
            _ => return fail("animation_clip_invalid", format!("clip '{}' needs exactly one of preset and prompt", c.name)),
        }
    }
    Ok(())
}

fn check_string(v: &str, spec: &crate::capabilities::ParamSpecString, param: &str) -> Result<(), ValidationError> {
    if let Some(m) = spec.max_length {
        if v.len() > m {
            return Err(ValidationError::new(
                "param_too_long",
                format!("{} length {} > {}", param, v.len(), m),
                Some(param),
            ));
        }
    }
    if !spec.enum_values.is_empty() && !spec.enum_values.iter().any(|e| e == v) {
        return Err(ValidationError::new(
            "param_enum_mismatch",
            format!("{} '{}' not in {:?}", param, v, spec.enum_values),
            Some(param),
        ));
    }
    check_pattern(v, spec, param)
}

/// N4: a string param's `pattern`, matched anywhere in the value as JSON
/// Schema's `pattern` is (`^…$` anchors it), in either mode.
fn check_pattern(v: &str, spec: &crate::capabilities::ParamSpecString, param: &str) -> Result<(), ValidationError> {
    match spec.pattern.as_deref() {
        Some(p) if !spec.matches_pattern(v) => Err(ValidationError::new(
            "param_invalid",
            format!("{param} '{}' does not match the pattern {p}", shown(v)),
            Some(param),
        )),
        _ => Ok(()),
    }
}

fn check_size(v: &str, spec: &SizeSpec, param: &str) -> Result<(), ValidationError> {
    let (w, h) = parse_size(v).ok_or_else(|| ValidationError::new(
        "param_enum_mismatch",
        format!("{} '{}' must be 'WxH'", param, v),
        Some(param),
    ))?;
    match spec {
        SizeSpec::Enum(e) => {
            if !e.values.iter().any(|(ww, hh)| *ww == w && *hh == h) {
                return Err(ValidationError::new(
                    "param_enum_mismatch",
                    format!("{} '{}' not in allowed sizes {:?}", param, v, e.values),
                    Some(param),
                ));
            }
        }
        SizeSpec::Freeform(f) => {
            if w < f.min_width || w > f.max_width || h < f.min_height || h > f.max_height {
                return Err(ValidationError::new(
                    "param_out_of_range",
                    format!("{} '{}' outside bounds", param, v),
                    Some(param),
                ));
            }
            if let Some(m) = f.multiple_of {
                if w % m != 0 || h % m != 0 {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("{} '{}' not divisible by {}", param, v, m),
                        Some(param),
                    ));
                }
            }
            let pixels = u64::from(w) * u64::from(h);
            if f.min_pixels.is_some_and(|min| pixels < min) || f.max_pixels.is_some_and(|max| pixels > max) {
                return Err(ValidationError::new(
                    "param_out_of_range",
                    format!(
                        "{param} '{v}' is {pixels} pixels; the model makes {}–{}",
                        f.min_pixels.unwrap_or(0),
                        f.max_pixels.map_or("any".to_string(), |m| m.to_string())
                    ),
                    Some(param),
                ));
            }
            let aspect = f64::from(w.max(h)) / f64::from(w.min(h).max(1));
            if let Some(max) = f.max_aspect.filter(|max| aspect > max + 1e-9) {
                return Err(ValidationError::new(
                    "param_out_of_range",
                    format!("{param} '{v}' is wider than the model's {max}:1"),
                    Some(param),
                ));
            }
        }
    }
    Ok(())
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    Some((w.parse().ok()?, h.parse().ok()?))
}

/// `n` (L13, E3): litegen makes one output per request, and a vendor asked
/// for more bills them all. Refused in both modes, since it is a bad value;
/// 0 is the same as 1. `one` names the output: "image", "video" or "mesh".
fn check_n(base: &BaseGenerationRequest, one: &str) -> Result<(), ValidationError> {
    if base.n > 1 {
        return Err(ValidationError::new(
            "param_out_of_range",
            format!("n {} > 1: litegen makes one {one} per request; send n: 1 or leave it out", base.n),
            Some("n"),
        ));
    }
    Ok(())
}

/// L13: what an image model cannot do is refused here, not at the vendor —
/// text alone on a model that only edits, an init image on one that takes
/// none. (`n` above one is `check_n`'s, for every family.)
fn check_image_request(schema: &ModelSchema, req: &ImageGenerationRequest) -> Result<(), ValidationError> {
    if schema.task != crate::capabilities::ModelTask::Generate {
        return Ok(());
    }
    let c = &schema.capabilities;
    let refs = &req.base.reference_images;
    if refs.is_empty() && !c.text_to_image {
        return Err(ValidationError::new(
            "capability_unsupported",
            format!("'{}' does not make an image from text alone; send an init image", schema.id),
            Some("reference_images"),
        ));
    }
    if refs.iter().any(|r| r.role.as_deref() == Some("init")) && !(c.image_to_image || c.inpainting || c.background_removal) {
        return Err(ValidationError::new(
            "capability_unsupported",
            format!("'{}' does not take an init image", schema.id),
            Some("reference_images"),
        ));
    }
    Ok(())
}

/// Video operations (3D doc §5, P2): a source for every task but `generate`;
/// `direction` and `replace` from the model's lists; a `time_range` the model
/// takes, a `retake` always — a window with a length, at least the model's
/// minimum, on its whole-second grid where it keeps one. A window past the
/// source's end is refused once the source is read, still before billing.
fn check_video_operation(
    schema: &ModelSchema,
    req: &mut VideoGenerationRequest,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let undeclared = |name: &str| ValidationError::new("param_unsupported", format!("{name} not supported by '{}'", schema.id), Some(name));
    match (&req.source_video, schema.params.get("source_video")) {
        (Some(src), Some(ParamSpec::SourceVideo(spec))) => {
            if src.kind != SourceKind::Generation {
                let format = src.format.as_deref().map(str::to_ascii_lowercase);
                if !format.as_ref().is_some_and(|f| spec.formats.iter().any(|v| v == f)) {
                    return Err(ValidationError::new(
                        "param_enum_mismatch",
                        format!("source_video format {:?}: '{}' takes {:?}", src.format, schema.id, spec.formats),
                        Some("source_video"),
                    ));
                }
            }
        }
        (Some(_), _) => {
            if strict {
                return Err(undeclared("source_video"));
            }
            dropped.push("source_video".into());
            req.source_video = None;
        }
        (None, Some(ParamSpec::SourceVideo(_))) if schema.task != ModelTask::Generate => {
            return Err(ValidationError::new(
                "source_video_required",
                format!("'{}' is a {} operation and needs source_video: the clip to work on", schema.id, schema.task.as_str()),
                Some("source_video"),
            ));
        }
        _ => {}
    }
    for (field, name) in [(&mut req.direction, "direction"), (&mut req.replace, "replace")] {
        let Some(v) = field.as_deref() else { continue };
        match schema.params.get(name) {
            Some(ParamSpec::String(s)) => check_string(v, s, name)?,
            _ if strict => return Err(undeclared(name)),
            _ => {
                dropped.push(name.into());
                *field = None;
            }
        }
    }
    match (req.time_range, schema.params.get("time_range")) {
        (Some(r), Some(ParamSpec::TimeRange(spec))) => {
            let bad = |msg: String| Err(ValidationError::new("param_out_of_range", msg, Some("time_range")));
            if !(r.start_s.is_finite() && r.end_s.is_finite()) || r.start_s < 0.0 || r.end_s <= r.start_s {
                return bad(format!("time_range {}–{} s is not a window", r.start_s, r.end_s));
            }
            if let Some(min) = spec.min_window_s.filter(|min| r.end_s - r.start_s < *min - 1e-9) {
                return bad(format!("'{}' retakes windows of at least {min} s", schema.id));
            }
            if spec.whole_seconds && (r.start_s.fract() != 0.0 || r.end_s.fract() != 0.0) {
                return bad(format!("'{}' takes windows on whole seconds", schema.id));
            }
        }
        (Some(_), _) => {
            if strict {
                return Err(undeclared("time_range"));
            }
            dropped.push("time_range".into());
            req.time_range = None;
        }
        (None, _) if schema.task == ModelTask::Retake => {
            return Err(ValidationError::new("param_required", format!("'{}' retakes a window: send time_range", schema.id), Some("time_range")));
        }
        _ => {}
    }
    Ok(())
}

/// PixelGenie L10: a still camera, a silent clip and a loop, where the model
/// declares each; strict mode refuses an undeclared one, lax mode drops it.
fn check_video_controls(
    schema: &ModelSchema,
    req: &mut VideoGenerationRequest,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let undeclared = |name: &str| ValidationError::new("param_unsupported", format!("'{}' does not take {name}", schema.id), Some(name));
    if let Some(camera) = req.camera.as_deref() {
        match schema.params.get("camera") {
            Some(ParamSpec::String(s)) => check_string(camera, s, "camera")?,
            _ if strict => return Err(undeclared("camera")),
            _ => {
                dropped.push("camera".into());
                req.camera = None;
            }
        }
    }
    for (name, value) in [("audio", &mut req.audio), ("loop", &mut req.r#loop)] {
        if value.is_some() && !matches!(schema.params.get(name), Some(ParamSpec::Bool(_))) {
            if strict {
                return Err(undeclared(name));
            }
            dropped.push(name.into());
            *value = None;
        }
    }
    Ok(())
}

/// L13: a video model without a text mode needs a frame, and one without an
/// image mode takes no frame — refused here, not at the vendor.
fn check_video_request(schema: &ModelSchema, req: &VideoGenerationRequest) -> Result<(), ValidationError> {
    if schema.task != crate::capabilities::ModelTask::Generate {
        return Ok(());
    }
    let c = &schema.capabilities;
    let refs = &req.base.reference_images;
    if refs.is_empty() && !c.text_to_video {
        return Err(ValidationError::new(
            "capability_unsupported",
            format!("'{}' does not make a video from text alone; send a first_frame image", schema.id),
            Some("reference_images"),
        ));
    }
    let frames = ["first_frame", "last_frame", "init", "keyframe"];
    if refs.iter().any(|r| r.role.as_deref().is_some_and(|role| frames.contains(&role))) && !c.image_to_video {
        return Err(ValidationError::new(
            "capability_unsupported",
            format!("'{}' makes video from text only; it takes no frame image", schema.id),
            Some("reference_images"),
        ));
    }
    Ok(())
}

/// Timed keyframes (3D doc §5). Only `keyframe` references carry a `time_s`,
/// and each sits within the clip — the request's duration, else the model's
/// default; a model with no duration param makes its clip from the keyframes —
/// at a time of its own, on the vendor's grid (`time_step_s`). An `even`
/// model's keyframes are evenly spaced from start to end: litegen spaces them
/// when every time is left out. They leave sorted by time.
fn check_keyframes(schema: &ModelSchema, req: &mut VideoGenerationRequest) -> Result<(), ValidationError> {
    const EPS: f64 = 1e-3;
    let refs = &mut req.base.reference_images;
    if let Some(r) = refs.iter().find(|r| r.time_s.is_some() && r.role.as_deref() != Some("keyframe")) {
        return Err(ValidationError::new(
            "param_unsupported",
            format!("time_s goes on keyframe references, not on '{}'", r.role.as_deref().unwrap_or("a reference without a role")),
            Some("reference_images"),
        ));
    }
    let Some(spec) = schema.ref_inputs.as_ref().and_then(|r| r.roles.get("keyframe")) else {
        return Ok(());
    };
    let slots: Vec<usize> = (0..refs.len()).filter(|&i| refs[i].role.as_deref() == Some("keyframe")).collect();
    if slots.is_empty() {
        return Ok(());
    }
    let duration = schema
        .params
        .contains_key("duration_seconds")
        .then(|| resolve_duration_seconds(schema, req.duration_seconds));
    let even = spec.timing == Some(crate::capabilities::KeyframeTiming::Even);
    if even && slots.iter().all(|&i| refs[i].time_s.is_none()) {
        if let Some(d) = duration {
            let n = slots.len();
            for (k, &i) in slots.iter().enumerate() {
                refs[i].time_s = Some(if n == 1 { 0.0 } else { d * k as f64 / (n - 1) as f64 });
            }
        }
    }
    let out_of_range = |msg: String| ValidationError::new("keyframe_time_out_of_range", msg, Some("reference_images"));
    for &i in &slots {
        let Some(t) = refs[i].time_s else {
            return Err(ValidationError::new(
                "keyframe_time_required",
                "every keyframe reference needs time_s: seconds from the start of the clip",
                Some("reference_images"),
            ));
        };
        if !t.is_finite() || t < 0.0 || duration.is_some_and(|d| t > d + EPS) {
            return Err(out_of_range(match duration {
                Some(d) => format!("keyframe time {t} s is outside the {d} s clip"),
                None => format!("keyframe time {t} s is negative"),
            }));
        }
        if let Some(max) = spec.max_time_s.filter(|max| duration.is_none() && t > max + EPS) {
            return Err(out_of_range(format!("keyframe time {t} s is past the {max} s '{}' makes at most", schema.id)));
        }
        if let Some(step) = spec.time_step_s {
            if ((t / step).round() * step - t).abs() > EPS {
                return Err(ValidationError::new(
                    "keyframe_off_grid",
                    format!("'{}' places keyframes on a {step} s grid; {t} s is not on it", schema.id),
                    Some("reference_images"),
                ));
            }
        }
    }
    let mut ordered: Vec<ReferenceImage> = slots.iter().map(|&i| refs[i].clone()).collect();
    ordered.sort_by(|a, b| a.time_s.unwrap_or_default().total_cmp(&b.time_s.unwrap_or_default()));
    if let Some(w) = ordered.windows(2).find(|w| (w[1].time_s.unwrap_or_default() - w[0].time_s.unwrap_or_default()).abs() <= EPS) {
        return Err(out_of_range(format!("two keyframes at {} s", w[0].time_s.unwrap_or_default())));
    }
    if even && ordered.len() > 1 {
        let (first, last) = (ordered[0].time_s.unwrap_or_default(), ordered[ordered.len() - 1].time_s.unwrap_or_default());
        let gap = (last - first) / (ordered.len() - 1) as f64;
        let uneven = first.abs() > EPS
            || duration.is_some_and(|d| (last - d).abs() > EPS)
            || ordered.iter().enumerate().any(|(k, r)| (r.time_s.unwrap_or_default() - (first + gap * k as f64)).abs() > EPS);
        if uneven {
            return Err(ValidationError::new(
                "keyframe_timing_uneven",
                format!("'{}' spaces keyframes evenly from the start of the clip to its end; leave time_s out, or send even times", schema.id),
                Some("reference_images"),
            ));
        }
    }
    for (&i, r) in slots.iter().zip(ordered) {
        refs[i] = r;
    }
    Ok(())
}

/// Multiview-to-3D (3D doc §3E). `view-*` references mean multiview, so they
/// cannot ride with an `init` image, which means image-to-3D; every vendor
/// needs the front; and a vendor that reads its views as one ordered list
/// (`ref_inputs.view_order`) cannot be sent a gap, because it would read the
/// next view in the missing one's place.
fn check_multiview_refs(schema: &ModelSchema, refs: &[ReferenceImage]) -> Result<(), ValidationError> {
    let roles: Vec<&str> = refs.iter().filter_map(|r| r.role.as_deref()).collect();
    let views: Vec<&str> = roles.iter().filter_map(|r| r.strip_prefix("view-")).collect();
    if views.is_empty() {
        return Ok(());
    }
    if roles.contains(&"init") {
        return Err(ValidationError::new(
            "ref_role_conflict",
            "send an init image (image-to-3D) or view-* images (multiview-to-3D), not both",
            Some("reference_images"),
        ));
    }
    if !views.contains(&"front") {
        return Err(ValidationError::new(
            "ref_role_required",
            "multiview-to-3D needs a view-front image",
            Some("reference_images"),
        ));
    }
    let order = schema.ref_inputs.as_ref().map(|r| r.view_order.as_slice()).unwrap_or_default();
    if let Some(last) = order.iter().rposition(|v| views.contains(&v.as_str())) {
        if let Some(missing) = order[..last].iter().find(|v| !views.contains(&v.as_str())) {
            return Err(ValidationError::new(
                "ref_role_required",
                format!(
                    "'{}' reads its views by position ({}): send view-{missing} too, or leave out the views after it",
                    schema.id,
                    order.join(", ")
                ),
                Some("reference_images"),
            ));
        }
    }
    Ok(())
}

fn check_refs(
    schema: &ModelSchema,
    refs: &mut Vec<ReferenceImage>,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let Some(ri) = schema.ref_inputs.as_ref() else {
        if !refs.is_empty() {
            if strict {
                return Err(ValidationError::new(
                    "ref_role_unknown",
                    format!("model '{}' does not accept reference images", schema.id),
                    Some("reference_images"),
                ));
            }
            for _ in refs.drain(..) { dropped.push("reference_images".into()); }
        }
        return Ok(());
    };

    if (refs.len() as u32) > ri.max_total {
        return Err(ValidationError::new(
            "ref_total_exceeded",
            format!("{} reference images > max {}", refs.len(), ri.max_total),
            Some("reference_images"),
        ));
    }

    // Resolve missing role → default_role; record dropped for unknown roles in lax.
    let mut keep: Vec<ReferenceImage> = Vec::with_capacity(refs.len());
    for r in refs.drain(..) {
        let role = r.role.clone().or_else(|| ri.default_role.clone());
        let Some(role_name) = role else {
            if strict {
                return Err(ValidationError::new(
                    "ref_role_unknown",
                    "reference image missing role and model has no default_role",
                    Some("reference_images"),
                ));
            }
            dropped.push("reference_images[no_role]".into());
            continue;
        };
        if !ri.roles.contains_key(&role_name) {
            if strict {
                return Err(ValidationError::new(
                    "ref_role_unknown",
                    format!("role '{}' not declared for model '{}'", role_name, schema.id),
                    Some("reference_images"),
                ));
            }
            dropped.push(format!("reference_images[{}]", role_name));
            continue;
        }
        keep.push(ReferenceImage { role: Some(role_name), ..r });
    }

    // Per-role count + required.
    let mut counts: HashMap<String, u32> = HashMap::new();
    for r in &keep {
        *counts.entry(r.role.clone().unwrap()).or_insert(0) += 1;
    }
    for (role, spec) in &ri.roles {
        let c = counts.get(role).copied().unwrap_or(0);
        // Absence is checked first, and independently of `min_count`: the
        // count branch below used to return for a required role before this
        // could, so a missing required image read as a count error, and a role
        // declared `required` with `min_count: 0` was never enforced at all.
        if spec.required && c == 0 {
            return Err(ValidationError::new(
                "ref_role_required",
                format!("role '{}' is required", role),
                Some("reference_images"),
            ));
        }
        if c > 0 && c < spec.min_count {
            return Err(ValidationError::new(
                "ref_role_count_out_of_range",
                format!("role '{}' count {} < min {}", role, c, spec.min_count),
                Some("reference_images"),
            ));
        }
        if c > spec.max_count {
            return Err(ValidationError::new(
                "ref_role_count_out_of_range",
                format!("role '{}' count {} > max {}", role, c, spec.max_count),
                Some("reference_images"),
            ));
        }
    }

    *refs = keep;
    Ok(())
}

/// An int param's range, then the values the row lists when the vendor takes
/// a list rather than a range (PixelGenie L10): out of range is
/// `param_out_of_range`, off the list `param_enum_mismatch` naming the values.
fn check_int(v: i64, s: &crate::capabilities::ParamSpecInt, key: &str, model: &str) -> Result<(), ValidationError> {
    if s.min.is_some_and(|m| v < m) || s.max.is_some_and(|m| v > m) {
        return Err(ValidationError::new("param_out_of_range", format!("{key} {v} out of range"), Some(key)));
    }
    if !s.values.is_empty() && !s.values.contains(&v) {
        return Err(ValidationError::new("param_enum_mismatch", format!("'{model}' takes {key} {:?}; got {v}", s.values), Some(key)));
    }
    Ok(())
}

/// [`check_int`] for a float param.
fn check_float(v: f64, s: &crate::capabilities::ParamSpecFloat, key: &str, model: &str) -> Result<(), ValidationError> {
    if s.min.is_some_and(|m| v < m) || s.max.is_some_and(|m| v > m) {
        return Err(ValidationError::new("param_out_of_range", format!("{key} {v} out of range"), Some(key)));
    }
    if !s.values.is_empty() && !s.values.iter().any(|a| (a - v).abs() < 1e-9) {
        return Err(ValidationError::new("param_enum_mismatch", format!("'{model}' takes {key} {:?}; got {v}", s.values), Some(key)));
    }
    Ok(())
}

/// `extra` keys that became a param under another name, so a caller from
/// before is told where it goes: Google's `imageSize` is `resolution` since
/// 2026-11-03.
const RENAMED_EXTRAS: &[(&str, &str)] = &[("imageSize", "resolution")];

/// Fields a body sent that its request type has no name for (E2), judged
/// once the model is known. `top` holds the unknown top-level keys with their
/// values, `nested` the unknown keys inside the request's objects (dotted
/// paths); `request` names the request schema (`openapi::REQUEST_SCHEMAS`).
///
/// A top-level key the model passes through (its `extra_allowlist`) is moved
/// into `extra`, as if sent there, in either mode: the OpenAI SDK sends
/// `user` and `moderation` at the top level. `check_extra` then judges it
/// like any `extra` key. Sent both ways with different values, it is refused
/// when strict (`param_invalid`), and `extra`'s value is kept otherwise.
/// Every other key is refused when strict (`param_unsupported`, naming at
/// most [`MAX_NAMED_FIELDS`]) and returned to be named as dropped otherwise.
pub fn check_unknown_fields(
    schema: &ModelSchema,
    request: &str,
    top: Vec<(String, Value)>,
    nested: Vec<String>,
    extra: &mut Option<Value>,
    strict: bool,
) -> Result<Vec<String>, ValidationError> {
    // A request with no `extra` (segmentation) has nowhere to forward to.
    let forwards = crate::api::openapi::request_fields(request).contains("extra");
    let mut names = Vec::new();
    for (key, value) in top {
        if !(forwards && schema.extra_allowlist.contains(&key)) {
            names.push(key);
            continue;
        }
        match extra {
            None => *extra = Some(Value::Object(serde_json::Map::from_iter([(key, value)]))),
            Some(Value::Object(map)) => match map.get(&key) {
                None => {
                    map.insert(key, value);
                }
                Some(sent) if *sent == value => {}
                Some(_) if strict => {
                    return Err(ValidationError::new(
                        "param_invalid",
                        format!("`{}` and `extra.{}` were both sent, with different values: send one", shown(&key), shown(&key)),
                        Some(&shown(&key)),
                    ));
                }
                // `extra`'s value goes to the vendor; the top-level one is dropped.
                Some(_) => names.push(key),
            },
            // No object: `check_extra` refuses it, in either mode.
            Some(_) => {}
        }
    }
    let at_top = names.len();
    names.extend(nested);
    if names.is_empty() || !strict {
        return Ok(names);
    }
    let listed: Vec<String> = names
        .iter()
        .take(MAX_NAMED_FIELDS)
        .enumerate()
        .map(|(i, name)| {
            let hint = if i < at_top { field_hint(request, name) } else { None };
            format!("`{}`{}", shown(name), hint.map(|h| format!(" ({h})")).unwrap_or_default())
        })
        .collect();
    let more = names.len().saturating_sub(MAX_NAMED_FIELDS);
    let (fields, it, more) = match (names.len(), more) {
        (1, _) => ("field", "it", String::new()),
        (_, 0) => ("fields", "them", String::new()),
        (_, n) => ("fields", "them", format!(", and {n} more")),
    };
    Err(ValidationError::new(
        "param_unsupported",
        format!(
            "no such {fields} on {} request: {}{more}. Leave {it} out, or send `strict: false` to have {it} dropped and named in X-Litegen-Dropped-Params",
            request_family(request),
            listed.join(", "),
        ),
        Some(&shown(&names[0])),
    ))
}

/// How many unknown fields a refusal names; the rest are counted. A 25 MiB
/// body of tiny keys would otherwise make a megabyte error, which `post_call`
/// hooks receive too.
pub const MAX_NAMED_FIELDS: usize = 20;

/// A caller's field name as a message shows it: at most 64 characters.
fn shown(name: &str) -> String {
    match name.char_indices().nth(64) {
        Some((cut, _)) => format!("{}...", &name[..cut]),
        None => name.to_string(),
    }
}

/// `request`'s family, with its article: "an image", "a video".
fn request_family(request: &str) -> &'static str {
    match request {
        "ImageGenerationRequest" => "an image",
        "VideoGenerationRequest" => "a video",
        "Model3dGenerationRequest" => "a 3D",
        "ImageSegmentationRequest" => "a segmentation",
        _ => "this",
    }
}

/// Where an unknown top-level field probably belongs: its snake_case form
/// (`durationSeconds`), a field it begins or is begun by (`duration`,
/// `prompt` for `prompts`), or another family's request.
fn field_hint(request: &str, key: &str) -> Option<String> {
    use crate::api::openapi::{request_fields, REQUEST_SCHEMAS};
    let known = request_fields(request);
    let snake = snake_case(key);
    if snake != key && known.contains(&snake) {
        return Some(format!("did you mean `{snake}`?"));
    }
    let begun = known.iter().find(|f| key.len() >= 3 && f.starts_with(key));
    let begins = known.iter().filter(|f| f.len() >= 3 && key.starts_with(f.as_str())).max_by_key(|f| f.len());
    if let Some(f) = begun.or(begins) {
        return Some(format!("did you mean `{f}`?"));
    }
    let families: Vec<&str> = REQUEST_SCHEMAS
        .iter()
        .filter(|r| **r != request && request_fields(r).contains(key))
        .map(|r| request_family(r))
        .collect();
    let (first, rest) = families.split_first()?;
    let rest: String = rest.iter().map(|f| format!(" or {}", f.split_once(' ').map_or(*f, |(_, noun)| noun))).collect();
    Some(format!("{first}{rest} request field"))
}

/// `durationSeconds`, `duration-seconds` and `Duration_Seconds` as
/// `duration_seconds`.
fn snake_case(key: &str) -> String {
    let mut out = String::with_capacity(key.len() + 4);
    let mut after_lower = false;
    for c in key.chars() {
        if c == '-' || c == ' ' {
            out.push('_');
            after_lower = false;
        } else if c.is_uppercase() {
            if after_lower {
                out.push('_');
            }
            out.extend(c.to_lowercase());
            after_lower = false;
        } else {
            out.push(c);
            after_lower = c.is_lowercase() || c.is_ascii_digit();
        }
    }
    out
}

/// The unknown fields of a request body (E2): the top-level ones taken out of
/// `json` with their values (serde ignores them anyway, and one the model
/// passes through goes on to `extra`), and the nested ones by path.
fn take_unknown_fields(request: &str, json: &mut Value) -> (Vec<(String, Value)>, Vec<String>) {
    let mut top = Vec::new();
    let mut nested = Vec::new();
    for field in crate::api::openapi::unknown_fields(request, json) {
        if field.top_level {
            let value = json.as_object_mut().and_then(|o| o.remove(&field.path)).unwrap_or(Value::Null);
            top.push((field.path, value));
        } else {
            nested.push(field.path);
        }
    }
    (top, nested)
}

fn check_extra(
    schema: &ModelSchema,
    extra: &mut Option<Value>,
    strict: bool,
    dropped: &mut Vec<String>,
) -> Result<(), ValidationError> {
    let Some(v) = extra else { return Ok(()); };
    let Some(obj) = v.as_object() else {
        return Err(ValidationError::new(
            "extra_key_unsupported",
            "extra must be a JSON object",
            Some("extra"),
        ));
    };
    // Only allowlisted keys reach the vendor, in either mode: strict refuses
    // the rest, lax drops and reports them (`extra.<key>`), as it does for
    // params. Passed through, a key could replace what the adapter sends —
    // `extra.model` ran a dearer model than the one billed.
    let mut filtered = serde_json::Map::new();
    for (k, val) in obj {
        if schema.extra_allowlist.iter().any(|a| a == k) {
            filtered.insert(k.clone(), val.clone());
        } else if strict {
            // A key that is one of the model's own params (`background` and
            // `output_format` were extra keys once) is told where it goes.
            let param = if schema.params.contains_key(k) {
                Some(k.as_str())
            } else {
                RENAMED_EXTRAS.iter().find(|(old, new)| old == k && schema.params.contains_key(*new)).map(|(_, new)| *new)
            };
            let hint = param
                .map(|p| format!("; it is a param of this model: send it as `{p}`"))
                // E4a: a field no row may allowlist says why.
                .or_else(|| crate::capabilities::gateway_owned_extra(k).map(|why| format!("; it is litegen's to set: {why}")))
                .unwrap_or_default();
            return Err(ValidationError::new(
                "extra_key_unsupported",
                format!("extra key '{}' not in allowlist for model '{}'{hint}", k, schema.id),
                Some(&format!("extra.{}", k)),
            ));
        } else {
            dropped.push(format!("extra.{k}"));
        }
    }
    // E4b: a typed extra is checked like a param, in either mode, as an
    // out-of-range param is; `null` and untyped keys pass as sent.
    for (k, val) in &filtered {
        let Some(why) = schema.extra_params.get(k).and_then(|spec| spec.extra_misfit(val)) else { continue };
        let code = match why {
            crate::capabilities::ExtraMisfit::Type(_) => "param_invalid",
            crate::capabilities::ExtraMisfit::Range(_) => "param_out_of_range",
            crate::capabilities::ExtraMisfit::NotListed(_) => "param_enum_mismatch",
            crate::capabilities::ExtraMisfit::TooLong(_) => "param_too_long",
            crate::capabilities::ExtraMisfit::TooMany(_) => "param_too_many",
            crate::capabilities::ExtraMisfit::Pattern(_) => "param_invalid",
        };
        let param = format!("extra.{k}");
        return Err(ValidationError::new(code, format!("{param} is {} for model '{}': {why}", shown(&val.to_string()), schema.id), Some(&param)));
    }
    *extra = Some(Value::Object(filtered));
    Ok(())
}

// ─── Axum extractor implementations ─────────────────────────────────────────

use axum::extract::{FromRequest, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::middleware::body::{BodyError, Buffered, MAX_GENERATION_BODY};
use crate::api::middleware::AppState;
// Named explicitly: the `crate::types` glob above exports a `MediaType` too.
use crate::capabilities::MediaType;
use crate::proxy::materializer::MaterializeContext;

pub struct ValidatedImage {
    pub schema: Arc<ModelSchema>,
    pub request: ImageGenerationRequest,
    pub dropped: Vec<String>,
    pub ctx: MaterializeContext,
    /// In-flight slot, taken once the body had arrived and held until the
    /// handler returns. Never read; exists only so the permit lives that long.
    pub _permit: tokio::sync::OwnedSemaphorePermit,
    /// The body, holding its share of the body budget until the handler
    /// returns (the request's parts, such as multipart files, live that long).
    pub _body: Buffered,
}

pub struct ValidatedVideo {
    pub schema: Arc<ModelSchema>,
    pub request: VideoGenerationRequest,
    pub dropped: Vec<String>,
    pub ctx: MaterializeContext,
    pub _permit: tokio::sync::OwnedSemaphorePermit,
    pub _body: Buffered,
}

#[derive(Debug)]
pub struct ValidationRejection(pub StatusCode, pub serde_json::Value);

impl IntoResponse for ValidationRejection {
    fn into_response(self) -> Response {
        let is_overloaded = self.0 == StatusCode::SERVICE_UNAVAILABLE;
        let mut resp = (self.0, Json(self.1)).into_response();
        if is_overloaded {
            resp.headers_mut()
                .insert("retry-after", axum::http::HeaderValue::from_static("1"));
        }
        resp
    }
}

/// 503 `server_at_capacity` (with `Retry-After: 1`).
fn at_capacity() -> ValidationRejection {
    ValidationRejection(
        StatusCode::SERVICE_UNAVAILABLE,
        err_body("server_at_capacity", "server at capacity, retry shortly", None, ""),
    )
}

/// Acquire an in-flight slot. Returns a 503 rejection (with `Retry-After: 1`)
/// when the server is at capacity.
fn acquire_in_flight(
    state: &Arc<AppState>,
) -> Result<tokio::sync::OwnedSemaphorePermit, ValidationRejection> {
    state.in_flight.try_acquire_owned().ok_or_else(at_capacity)
}

/// A body that was not read, in the API's error shape: 413, 408, 503 or 400.
fn body_rejection(e: BodyError) -> ValidationRejection {
    ValidationRejection(e.status(), err_body(e.code(), &e.message(), None, ""))
}

/// The body of a generation, quote or segmentation request, then its
/// in-flight slot. A server with every slot taken refuses before reading.
/// Otherwise the body is read first, each byte charged to the body budget as
/// it arrives (`body.rs`), so a client that sends slowly holds only what it
/// sent, never a slot; then, once it is all there, the slot is taken.
async fn body_then_slot(
    req: Request,
    state: &Arc<AppState>,
) -> Result<(axum::http::request::Parts, Buffered, tokio::sync::OwnedSemaphorePermit), ValidationRejection> {
    if state.in_flight.is_full() {
        return Err(at_capacity());
    }
    let (parts, body) = state.in_flight.body().read_request(req, MAX_GENERATION_BODY).await.map_err(body_rejection)?;
    let permit = acquire_in_flight(state)?;
    Ok((parts, body, permit))
}

/// The answer for a model id nothing serves: 410 `model_retired` for a row
/// past its retirement, naming its replacement (`replaced_by`), else 404
/// `model_not_found`.
pub(crate) fn unserved_model(state: &AppState, id: &str) -> ValidationRejection {
    match state.registry.retired(id) {
        Some(m) => ValidationRejection(StatusCode::GONE, retired_body(&m)),
        None => ValidationRejection(StatusCode::NOT_FOUND, err_body("model_not_found", &format!("model '{id}' not found"), None, id)),
    }
}

/// `model_retired`'s body: when the model retired, and what replaces it.
pub(crate) fn retired_body(m: &ModelSchema) -> serde_json::Value {
    let on = m.retires_at.as_deref().map(|d| format!(" after {d}")).unwrap_or_default();
    let instead = m.replaced_by.as_deref().map(|r| format!("; use '{r}' instead")).unwrap_or_default();
    let mut body = err_body("model_retired", &format!("model '{}' was retired{on}{instead}", m.id), None, &m.id);
    if let Some(r) = &m.replaced_by {
        body["error"]["replaced_by"] = serde_json::Value::from(r.clone());
    }
    body
}

pub fn err_body(code: &str, msg: &str, param: Option<&str>, model: &str) -> serde_json::Value {
    serde_json::json!({
        "error": {
            "type": "validation_error",
            "code": code,
            "message": msg,
            "param": param,
            "model": model,
        }
    })
}

/// Reject a model from another media family — e.g. a mesh model posted to
/// `/v1/images/cost` — naming the endpoint that does serve it. Every
/// `Validated*` extractor runs this right after the schema lookup and BEFORE
/// `validate_*`: without it the request is dispatched to the wrong family's
/// provider (params permitting), or rejected for some param the other family
/// happens to lack, which hides the actual mistake.
pub(crate) fn require_media_type(schema: &ModelSchema, served: MediaType) -> Result<(), ValidationRejection> {
    if schema.media_type == served {
        return Ok(());
    }
    let (family, endpoint) = match schema.media_type {
        MediaType::Image => ("an image", "/v1/images/generations"),
        MediaType::Video => ("a video", "/v1/videos/generations"),
        MediaType::Model3d => ("a model3d", "/v1/models3d/generations"),
    };
    Err(ValidationRejection(
        StatusCode::BAD_REQUEST,
        err_body(
            "model_media_type_mismatch",
            &format!("model '{}' is {} model; use {}", schema.id, family, endpoint),
            Some("model"),
            &schema.id,
        ),
    ))
}

async fn parse_request_and_context(
    headers: &HeaderMap,
    body: axum::body::Bytes,
) -> Result<(serde_json::Value, MaterializeContext), ValidationRejection> {
    let ct = headers.get(axum::http::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("application/json");
    if ct.starts_with("multipart/form-data") {
        let boundary = ct.split(';')
            .find_map(|p| p.trim().strip_prefix("boundary="))
            .ok_or_else(|| ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_multipart", "missing boundary", None, "")))?
            .to_string();
        let mut multipart = multer::Multipart::new(
            futures::stream::once(async move { Ok::<_, std::convert::Infallible>(body) }),
            boundary,
        );
        let mut req_json: Option<serde_json::Value> = None;
        let mut blob_parts: std::collections::HashMap<String, (bytes::Bytes, String)> = Default::default();
        while let Some(part) = multipart.next_field().await.map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_multipart", &e.to_string(), None, ""))
        })? {
            let name = part.name().unwrap_or("").to_string();
            let part_ct = part.content_type().map(|s| s.to_string()).unwrap_or_else(|| "application/octet-stream".into());
            let bytes = part.bytes().await.map_err(|e| {
                ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_multipart", &e.to_string(), None, ""))
            })?;
            if name == "request" {
                req_json = Some(serde_json::from_slice(&bytes).map_err(|e| {
                    ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
                })?);
            } else {
                blob_parts.insert(name, (bytes, part_ct));
            }
        }
        let json = req_json.ok_or_else(|| ValidationRejection(
            StatusCode::BAD_REQUEST,
            err_body("malformed_multipart", "missing 'request' part", None, ""),
        ))?;
        Ok((json, MaterializeContext { blob_parts }))
    } else {
        let json: serde_json::Value = serde_json::from_slice(&body).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        Ok((json, MaterializeContext::default()))
    }
}

impl FromRequest<Arc<AppState>> for ValidatedImage {
    type Rejection = ValidationRejection;
    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        // The body first, within the body budget and its deadline; the
        // in-flight slot once it is all there (`body_then_slot`).
        let (parts, body, permit) = body_then_slot(req, state).await?;
        let headers = parts.headers;
        let (mut json, ctx) = parse_request_and_context(&headers, body.bytes.clone()).await?;
        let (top, nested) = take_unknown_fields(IMAGE_REQUEST, &mut json);
        let mut req: ImageGenerationRequest = serde_json::from_value(json).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        let schema = state.registry.resolve(&req.base.model)
            .ok_or_else(|| unserved_model(state, &req.base.model))?;
        let schema = Arc::new(schema);
        require_media_type(&schema, MediaType::Image)?;
        let refuse = |e: ValidationError| ValidationRejection(StatusCode::BAD_REQUEST, err_body(&e.code, &e.message, e.param.as_deref(), &schema.id));
        require_task(&schema, ModelTask::Generate).map_err(refuse)?;
        // Unknown fields after the model, family and task checks, so those
        // mistakes are named first, and before the params, so a field that
        // does not exist is named before anything about its value.
        let strict = req.base.strict;
        let mut dropped = check_unknown_fields(&schema, IMAGE_REQUEST, top, nested, &mut req.base.extra, strict).map_err(refuse)?;
        let out = validate_image(&schema, req).map_err(refuse)?;
        dropped.extend(out.dropped);
        Ok(ValidatedImage { schema, request: out.request, dropped, ctx, _permit: permit, _body: body })
    }
}

/// The request schemas' names, as `openapi::REQUEST_SCHEMAS` has them.
const IMAGE_REQUEST: &str = "ImageGenerationRequest";
const VIDEO_REQUEST: &str = "VideoGenerationRequest";
const MODEL3D_REQUEST: &str = "Model3dGenerationRequest";
const SEGMENTATION_REQUEST: &str = "ImageSegmentationRequest";

impl FromRequest<Arc<AppState>> for ValidatedVideo {
    type Rejection = ValidationRejection;
    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        // The body, then the slot (see ValidatedImage).
        let (parts, body, permit) = body_then_slot(req, state).await?;
        let headers = parts.headers;
        let (mut json, ctx) = parse_request_and_context(&headers, body.bytes.clone()).await?;
        let (top, nested) = take_unknown_fields(VIDEO_REQUEST, &mut json);
        let mut req: VideoGenerationRequest = serde_json::from_value(json).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        let schema = state.registry.resolve(&req.base.model)
            .ok_or_else(|| unserved_model(state, &req.base.model))?;
        let schema = Arc::new(schema);
        require_media_type(&schema, MediaType::Video)?;
        let refuse = |e: ValidationError| ValidationRejection(StatusCode::BAD_REQUEST, err_body(&e.code, &e.message, e.param.as_deref(), &schema.id));
        // Unknown fields between the model checks and the params (see ValidatedImage).
        let strict = req.base.strict;
        let mut dropped = check_unknown_fields(&schema, VIDEO_REQUEST, top, nested, &mut req.base.extra, strict).map_err(refuse)?;
        let out = validate_video(&schema, req).map_err(refuse)?;
        dropped.extend(out.dropped);
        Ok(ValidatedVideo { schema, request: out.request, dropped, ctx, _permit: permit, _body: body })
    }
}

pub struct ValidatedModel3d {
    pub schema: Arc<ModelSchema>,
    pub request: Model3dGenerationRequest,
    pub dropped: Vec<String>,
    pub ctx: MaterializeContext,
    pub _permit: tokio::sync::OwnedSemaphorePermit,
    pub _body: Buffered,
}

impl FromRequest<Arc<AppState>> for ValidatedModel3d {
    type Rejection = ValidationRejection;
    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        // The body, then the slot (see ValidatedImage).
        let (parts, body, permit) = body_then_slot(req, state).await?;
        let headers = parts.headers;
        let (mut json, ctx) = parse_request_and_context(&headers, body.bytes.clone()).await?;
        let (top, nested) = take_unknown_fields(MODEL3D_REQUEST, &mut json);
        let mut req: Model3dGenerationRequest = serde_json::from_value(json).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        let schema = state.registry.resolve(&req.base.model)
            .ok_or_else(|| unserved_model(state, &req.base.model))?;
        let schema = Arc::new(schema);
        require_media_type(&schema, MediaType::Model3d)?;
        let refuse = |e: ValidationError| ValidationRejection(StatusCode::BAD_REQUEST, err_body(&e.code, &e.message, e.param.as_deref(), &schema.id));
        // Unknown fields between the model checks and the params (see ValidatedImage).
        let strict = req.base.strict;
        let mut dropped = check_unknown_fields(&schema, MODEL3D_REQUEST, top, nested, &mut req.base.extra, strict).map_err(refuse)?;
        let out = validate_model3d(&schema, req).map_err(refuse)?;
        dropped.extend(out.dropped);
        Ok(ValidatedModel3d { schema, request: out.request, dropped, ctx, _permit: permit, _body: body })
    }
}

/// A segmentation request checked against its model and its image
/// (PixelGenie L7), ready for the vendor.
pub struct ValidatedSegmentation {
    pub schema: Arc<ModelSchema>,
    pub input: crate::providers::SegmentationInput,
    /// The caller's `metadata`, validated.
    pub metadata: Option<serde_json::Value>,
    /// Fields dropped because `strict` was false (E2), for
    /// `X-Litegen-Dropped-Params`.
    pub dropped: Vec<String>,
    pub _permit: tokio::sync::OwnedSemaphorePermit,
    pub _body: Buffered,
}

impl FromRequest<Arc<AppState>> for ValidatedSegmentation {
    type Rejection = ValidationRejection;
    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        // The body, then the slot (see ValidatedImage).
        let (_, body, permit) = body_then_slot(req, state).await?;
        let malformed = |e: serde_json::Error| ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""));
        let mut json: Value = serde_json::from_slice(&body.bytes).map_err(malformed)?;
        let (top, nested) = take_unknown_fields(SEGMENTATION_REQUEST, &mut json);
        let req: ImageSegmentationRequest = serde_json::from_value(json).map_err(malformed)?;
        let schema = state.registry.resolve(&req.model)
            .ok_or_else(|| unserved_model(state, &req.model))?;
        let schema = Arc::new(schema);
        require_media_type(&schema, MediaType::Image)?;
        let refuse = |e: ValidationError| ValidationRejection(StatusCode::BAD_REQUEST, err_body(&e.code, &e.message, e.param.as_deref(), &schema.id));
        require_task(&schema, ModelTask::Segment).map_err(refuse)?;
        // Unknown fields between the model checks and the prompts (see
        // ValidatedImage). A segmentation request has no `extra` to forward to.
        let dropped = check_unknown_fields(&schema, SEGMENTATION_REQUEST, top, nested, &mut None, req.strict).map_err(refuse)?;
        validate_segmentation(&schema, &req).map_err(refuse)?;
        let image = req.image;
        let (image, content_type, width, height) = tokio::task::spawn_blocking(move || crate::proxy::segmentation::read_image(&image))
            .await
            .map_err(|e| {
                // A panic's text names code: the log has it, not the caller (SEC-DBERR).
                tracing::error!(error = %e, "reading a segmentation image failed");
                ValidationRejection(StatusCode::INTERNAL_SERVER_ERROR, err_body("internal_error", "internal server error", None, &schema.id))
            })?
            .map_err(|msg| refuse(ValidationError::new("image_unreadable", msg, Some("image"))))?;
        check_segmentation_bounds(&req.prompts, width, height).map_err(refuse)?;
        let max_masks = match schema.params.get("prompts") {
            Some(ParamSpec::SegmentationPrompts(p)) => p.max_masks,
            _ => 1,
        };
        let input = crate::providers::SegmentationInput {
            image,
            content_type: content_type.to_string(),
            width,
            height,
            prompts: req.prompts,
            max_masks,
        };
        Ok(ValidatedSegmentation { schema, input, metadata: req.metadata.filter(|m| !m.is_null()), dropped, _permit: permit, _body: body })
    }
}

/// How many names `X-Litegen-Dropped-Params` lists; the rest are counted.
pub const MAX_DROPPED_NAMES: usize = 32;

/// `X-Litegen-Dropped-Params`: the dropped names, comma-separated, `None`
/// when nothing was dropped. The value stays visible ASCII, which the MCP
/// note and the SDKs need (`to_str()`): a name is listed only when it is 1–64
/// visible ASCII characters with no comma, at most [`MAX_DROPPED_NAMES`] of
/// them, and every other name is counted in a last `+N more` (N2). One
/// unsafe name used to delete the whole header.
pub fn dropped_header(values: &[String]) -> Option<(axum::http::HeaderName, HeaderValue)> {
    if values.is_empty() {
        return None;
    }
    let safe = |name: &str| (1..=64).contains(&name.len()) && name.bytes().all(|b| b.is_ascii_graphic() && b != b',');
    let mut listed: Vec<&str> = Vec::new();
    let mut more = 0usize;
    for name in values {
        if listed.len() < MAX_DROPPED_NAMES && safe(name) {
            listed.push(name);
        } else {
            more += 1;
        }
    }
    let counted = (more > 0).then(|| format!("+{more} more"));
    let value = listed.into_iter().map(str::to_string).chain(counted).collect::<Vec<_>>().join(",");
    let value = HeaderValue::from_str(&value).expect("visible ASCII names and a count make a header value");
    Some((axum::http::HeaderName::from_static("x-litegen-dropped-params"), value))
}
