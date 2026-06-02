// litegen-core/src/api/middleware/validator.rs
use serde_json::Value;
use std::collections::HashMap;
use std::sync::Arc;

use crate::capabilities::*;
use crate::types::*;

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

#[tracing::instrument(skip(schema, req), fields(model = %schema.id))]
pub fn validate_image(
    schema: &ModelSchema,
    mut req: ImageGenerationRequest,
) -> Result<ImageValidationOutput, ValidationError> {
    let mut dropped = Vec::new();
    let strict = req.base.strict;

    check_prompt(&schema.prompt, &req.base.prompt)?;

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
    });

    check_param!(req.steps, "steps", ParamSpec::Int(s) => {
        let v = req.steps.unwrap() as i64;
        if s.min.map(|m| v < m).unwrap_or(false) || s.max.map(|m| v > m).unwrap_or(false) {
            return Err(ValidationError::new(
                "param_out_of_range",
                format!("steps {} out of range", v),
                Some("steps"),
            ));
        }
    });

    check_param!(req.guidance_scale, "guidance_scale", ParamSpec::Float(s) => {
        let v = req.guidance_scale.unwrap();
        if s.min.map(|m| v < m).unwrap_or(false) || s.max.map(|m| v > m).unwrap_or(false) {
            return Err(ValidationError::new(
                "param_out_of_range",
                format!("guidance_scale {} out of range", v),
                Some("guidance_scale"),
            ));
        }
    });

    check_param!(req.strength, "strength", ParamSpec::Float(s) => {
        let v = req.strength.unwrap();
        if s.min.map(|m| v < m).unwrap_or(false) || s.max.map(|m| v > m).unwrap_or(false) {
            return Err(ValidationError::new(
                "param_out_of_range",
                format!("strength {} out of range", v),
                Some("strength"),
            ));
        }
    });

    check_param!(req.quality, "quality", ParamSpec::String(s) => {
        let v = req.quality.as_deref().unwrap();
        check_string(v, &s.enum_values, s.max_length, "quality")?;
    });

    check_param!(req.style, "style", ParamSpec::String(s) => {
        let v = req.style.as_deref().unwrap();
        check_string(v, &s.enum_values, s.max_length, "style")?;
    });

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

    check_prompt(&schema.prompt, &req.base.prompt)?;

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

    // duration_seconds is special — float typically; an Int spec is also accepted.
    if req.duration_seconds > 0.0 {
        match schema.params.get("duration_seconds") {
            Some(ParamSpec::Float(s)) => {
                let v = req.duration_seconds;
                if s.min.map(|m| v < m).unwrap_or(false) || s.max.map(|m| v > m).unwrap_or(false) {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("duration_seconds {} out of range", v),
                        Some("duration_seconds"),
                    ));
                }
            }
            Some(ParamSpec::Int(s)) => {
                let v = req.duration_seconds.round() as i64;
                if s.min.map(|m| v < m).unwrap_or(false) || s.max.map(|m| v > m).unwrap_or(false) {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("duration_seconds {} out of range", v),
                        Some("duration_seconds"),
                    ));
                }
            }
            _ => {} // no spec → accept default; provider may further constrain
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
            Some(ParamSpec::Int(s)) => {
                let v = req.fps.unwrap() as i64;
                if s.min.map(|m| v < m).unwrap_or(false) || s.max.map(|m| v > m).unwrap_or(false) {
                    return Err(ValidationError::new(
                        "param_out_of_range",
                        format!("fps {} out of range", v),
                        Some("fps"),
                    ));
                }
            }
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
                check_string(v, &s.enum_values, s.max_length, "resolution")?;
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

    check_refs(schema, &mut req.base.reference_images, strict, &mut dropped)?;
    check_extra(schema, &mut req.base.extra, strict, &mut dropped)?;

    Ok(VideoValidationOutput { request: req, dropped })
}

fn check_prompt(spec: &PromptSpec, value: &str) -> Result<(), ValidationError> {
    if value.trim().is_empty() {
        if spec.required {
            return Err(ValidationError::new(
                "prompt_required",
                "prompt is required",
                Some("prompt"),
            ));
        }
        return Ok(());
    }
    if let Some(min) = spec.min_length {
        if value.len() < min {
            return Err(ValidationError::new(
                "prompt_too_short",
                format!("prompt length {} < {}", value.len(), min),
                Some("prompt"),
            ));
        }
    }
    if let Some(max) = spec.max_length {
        if value.len() > max {
            return Err(ValidationError::new(
                "prompt_too_long",
                format!("prompt length {} > {}", value.len(), max),
                Some("prompt"),
            ));
        }
    }
    Ok(())
}

fn check_string(
    v: &str,
    enum_values: &[String],
    max_length: Option<usize>,
    param: &str,
) -> Result<(), ValidationError> {
    if let Some(m) = max_length {
        if v.len() > m {
            return Err(ValidationError::new(
                "param_too_long",
                format!("{} length {} > {}", param, v.len(), m),
                Some(param),
            ));
        }
    }
    if !enum_values.is_empty() && !enum_values.iter().any(|e| e == v) {
        return Err(ValidationError::new(
            "param_enum_mismatch",
            format!("{} '{}' not in {:?}", param, v, enum_values),
            Some(param),
        ));
    }
    Ok(())
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
        }
    }
    Ok(())
}

fn parse_size(s: &str) -> Option<(u32, u32)> {
    let (w, h) = s.split_once('x').or_else(|| s.split_once('X'))?;
    Some((w.parse().ok()?, h.parse().ok()?))
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
        if c < spec.min_count {
            if spec.required || c > 0 {
                return Err(ValidationError::new(
                    "ref_role_count_out_of_range",
                    format!("role '{}' count {} < min {}", role, c, spec.min_count),
                    Some("reference_images"),
                ));
            }
            if spec.required && c == 0 {
                return Err(ValidationError::new(
                    "ref_role_required",
                    format!("role '{}' is required", role),
                    Some("reference_images"),
                ));
            }
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
    if !strict {
        return Ok(());
    }
    let mut filtered = serde_json::Map::new();
    for (k, val) in obj {
        if schema.extra_allowlist.iter().any(|a| a == k) {
            filtered.insert(k.clone(), val.clone());
        } else {
            return Err(ValidationError::new(
                "extra_key_unsupported",
                format!("extra key '{}' not in allowlist for model '{}'", k, schema.id),
                Some(&format!("extra.{}", k)),
            ));
        }
    }
    *extra = Some(Value::Object(filtered));
    let _ = dropped; // currently strict mode either fails or keeps; lax leaves unchanged.
    Ok(())
}

// ─── Axum extractor implementations ─────────────────────────────────────────

use axum::extract::{FromRequest, Request};
use axum::http::{HeaderMap, HeaderValue, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;

use crate::api::middleware::AppState;
use crate::proxy::materializer::MaterializeContext;

pub struct ValidatedImage {
    pub schema: Arc<ModelSchema>,
    pub request: ImageGenerationRequest,
    pub dropped: Vec<String>,
    pub ctx: MaterializeContext,
}

pub struct ValidatedVideo {
    pub schema: Arc<ModelSchema>,
    pub request: VideoGenerationRequest,
    pub dropped: Vec<String>,
    pub ctx: MaterializeContext,
}

#[derive(Debug)]
pub struct ValidationRejection(pub StatusCode, pub serde_json::Value);

impl IntoResponse for ValidationRejection {
    fn into_response(self) -> Response {
        (self.0, Json(self.1)).into_response()
    }
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
        let (parts, body) = req.into_parts();
        let headers = parts.headers;
        let bytes = axum::body::to_bytes(body, 25 * 1024 * 1024).await.map_err(|e| {
            ValidationRejection(StatusCode::PAYLOAD_TOO_LARGE, err_body("body_too_large", &e.to_string(), None, ""))
        })?;
        let (json, ctx) = parse_request_and_context(&headers, bytes).await?;
        let req: ImageGenerationRequest = serde_json::from_value(json).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        let schema = state.registry.get(&req.base.model)
            .ok_or_else(|| ValidationRejection(StatusCode::NOT_FOUND, err_body("model_not_found", &format!("model '{}' not found", req.base.model), None, &req.base.model)))?
            .clone();
        let schema = Arc::new(schema);
        match validate_image(&schema, req) {
            Ok(out) => Ok(ValidatedImage { schema, request: out.request, dropped: out.dropped, ctx }),
            Err(e) => Err(ValidationRejection(
                StatusCode::BAD_REQUEST,
                err_body(&e.code, &e.message, e.param.as_deref(), &schema.id),
            )),
        }
    }
}

impl FromRequest<Arc<AppState>> for ValidatedVideo {
    type Rejection = ValidationRejection;
    async fn from_request(req: Request, state: &Arc<AppState>) -> Result<Self, Self::Rejection> {
        let (parts, body) = req.into_parts();
        let headers = parts.headers;
        let bytes = axum::body::to_bytes(body, 25 * 1024 * 1024).await.map_err(|e| {
            ValidationRejection(StatusCode::PAYLOAD_TOO_LARGE, err_body("body_too_large", &e.to_string(), None, ""))
        })?;
        let (json, ctx) = parse_request_and_context(&headers, bytes).await?;
        let req: VideoGenerationRequest = serde_json::from_value(json).map_err(|e| {
            ValidationRejection(StatusCode::BAD_REQUEST, err_body("malformed_request", &e.to_string(), None, ""))
        })?;
        let schema = state.registry.get(&req.base.model)
            .ok_or_else(|| ValidationRejection(StatusCode::NOT_FOUND, err_body("model_not_found", &format!("model '{}' not found", req.base.model), None, &req.base.model)))?
            .clone();
        let schema = Arc::new(schema);
        match validate_video(&schema, req) {
            Ok(out) => Ok(ValidatedVideo { schema, request: out.request, dropped: out.dropped, ctx }),
            Err(e) => Err(ValidationRejection(
                StatusCode::BAD_REQUEST,
                err_body(&e.code, &e.message, e.param.as_deref(), &schema.id),
            )),
        }
    }
}

pub fn dropped_header(values: &[String]) -> Option<(axum::http::HeaderName, HeaderValue)> {
    if values.is_empty() { return None; }
    let s = values.join(",");
    Some((axum::http::HeaderName::from_static("x-litegen-dropped-params"), HeaderValue::from_str(&s).ok()?))
}
