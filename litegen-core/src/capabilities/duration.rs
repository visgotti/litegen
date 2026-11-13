// The duration a video request sends its provider, from the catalog's own
// declared default. Catalog logic: pricing and the request validator both
// use it (moved from api/middleware/validator.rs, docs/superpowers/specs/
// 2026-11-07-engine-boundary-design.md D5).

use crate::capabilities::{ModelSchema, ParamSpec};

/// The duration to send a provider: what the caller asked for, else the model's
/// own declared default, else the legacy 5s for models that declare no duration
/// param. An omitted duration must not become a fixed 5s — models whose minimum
/// is higher (fal/ltx-2.3 and bedrock nova-reel at 6s) reject that.
///
/// A request of 0 s or less, or no finite number, is no clip at all: the
/// request validator refuses it, and here it counts as omitted, so a path that
/// skips the validator prices the default clip the vendor makes, never a $0
/// one (E1).
pub fn resolve_duration_seconds(schema: &ModelSchema, requested: Option<f64>) -> f64 {
    if let Some(v) = requested.filter(|v| v.is_finite() && *v > 0.0) {
        return v;
    }
    match schema.params.get("duration_seconds") {
        Some(ParamSpec::Float(s)) => s.default.unwrap_or(5.0),
        Some(ParamSpec::Int(s)) => s.default.map(|d| d as f64).unwrap_or(5.0),
        _ => 5.0,
    }
}
