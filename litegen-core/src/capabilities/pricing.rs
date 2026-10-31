//! Prices that match the vendor's bill (3D doc §3G, L14).
//!
//! A row's price is `base_cost_usd`, the vendor's price for its default
//! request, run through its `variable_pricing` rules for the request at hand.
//! [`unit_price_usd`] prices the quote (`/cost`), the quota reservation and
//! the charge alike, from the same [`PriceFacts`], so the three always agree.

use serde_json::{Map, Value};

use super::{ModelSchema, ParamSpec, PriceRule};
use crate::types::BaseGenerationRequest;

/// Derived facts a rule may name besides the model's params.
pub const DERIVED_FACTS: &[&str] = &["views", "keyframes", "animations", "duration_seconds", "megapixels"];

/// What a price reads about one request: every param it sent, else the
/// model's default for it; `extra.<field>` for each vendor field; and the
/// derived facts — the `views`, `keyframes` and `animations` counts, the
/// clip's `duration_seconds` and the image's `megapixels`.
#[derive(Debug, Clone, Default)]
pub struct PriceFacts(Map<String, Value>);

impl PriceFacts {
    /// Facts from a validated request, or from the extras built from one:
    /// `params` is either, serialised (their priced fields share names).
    pub fn new(schema: &ModelSchema, base: &BaseGenerationRequest, params: &Value) -> Self {
        let mut facts: Map<String, Value> = params
            .as_object()
            .map(|o| o.iter().filter(|(_, v)| !v.is_null()).map(|(k, v)| (k.clone(), v.clone())).collect())
            .unwrap_or_default();
        for (name, spec) in &schema.params {
            if !facts.contains_key(name) {
                if let Some(default) = param_default(spec) {
                    facts.insert(name.clone(), default);
                }
            }
        }
        if let Some(Value::Object(extra)) = facts.remove("extra").or_else(|| base.extra.clone()) {
            for (k, v) in extra {
                facts.insert(format!("extra.{k}"), v);
            }
        }
        let roles = || base.reference_images.iter().filter_map(|r| r.role.as_deref());
        let views = roles().filter(|r| r.starts_with("view-")).count();
        let keyframes: Vec<f64> =
            base.reference_images.iter().filter(|r| r.role.as_deref() == Some("keyframe")).filter_map(|r| r.time_s).collect();
        if views > 0 {
            facts.insert("views".into(), Value::from(views));
        }
        if !keyframes.is_empty() {
            facts.insert("keyframes".into(), Value::from(keyframes.len()));
        }
        if let Some(Value::Array(clips)) = facts.get("animations") {
            let n = clips.len();
            facts.insert("animations".into(), Value::from(n));
        }
        // The clip's length: the request's, else the model's default; a
        // model with no duration param makes its clip from its keyframes.
        let duration = if schema.params.contains_key("duration_seconds") || keyframes.is_empty() {
            crate::api::middleware::validator::resolve_duration_seconds(schema, facts.get("duration_seconds").and_then(Value::as_f64))
        } else {
            keyframes.iter().copied().fold(0.0, f64::max)
        };
        facts.insert("duration_seconds".into(), Value::from(duration));
        let megapixels = facts
            .get("size")
            .and_then(Value::as_str)
            .and_then(|s| s.split_once(['x', 'X']))
            .and_then(|(w, h)| Some(w.trim().parse::<f64>().ok()? * h.trim().parse::<f64>().ok()? / 1_000_000.0))
            .unwrap_or(1.0);
        facts.insert("megapixels".into(), Value::from(megapixels));
        Self(facts)
    }

    fn get(&self, name: &str) -> Option<&Value> {
        self.0.get(name)
    }

    fn number(&self, name: &str) -> f64 {
        self.get(name).and_then(Value::as_f64).unwrap_or(0.0)
    }
}

/// A param's declared default, as a request would send it.
fn param_default(spec: &ParamSpec) -> Option<Value> {
    match spec {
        ParamSpec::Bool(s) => s.default.map(Value::from),
        ParamSpec::Int(s) => s.default.map(Value::from),
        ParamSpec::Float(s) => s.default.map(Value::from),
        ParamSpec::String(s) => s.default.clone().map(Value::from),
        ParamSpec::StringArray(s) if !s.default.is_empty() => Some(Value::from(s.default.clone())),
        _ => None,
    }
}

/// Whether a fact has the value a rule asks for: equal (numbers by value),
/// or present at all for `"*"`.
fn matches(fact: Option<&Value>, want: &Value) -> bool {
    match (fact, want) {
        (Some(_), Value::String(s)) if s == "*" => true,
        (Some(Value::Number(a)), Value::Number(b)) => a.as_f64() == b.as_f64(),
        (Some(a), b) => a == b,
        (None, _) => false,
    }
}

impl PriceRule {
    fn applies(&self, facts: &PriceFacts) -> bool {
        self.when.iter().all(|(name, want)| matches(facts.get(name), want))
    }

    fn apply(&self, price: f64, facts: &PriceFacts) -> f64 {
        if let Some(usd) = self.usd {
            usd
        } else if let Some(rate) = self.per_second_usd {
            rate * facts.number("duration_seconds")
        } else if let Some(rate) = self.per_megapixel_usd {
            rate * facts.number("megapixels")
        } else if let (Some(rate), Some(list)) = (self.per_item_usd, self.per_item.as_deref()) {
            rate * facts.number(list)
        } else if let Some(k) = self.multiply {
            price * k
        } else if let Some(anchor) = self.multiply_per_megapixel_above {
            price * (facts.number("megapixels") / anchor).max(1.0)
        } else if let Some(usd) = self.add_usd {
            price + usd
        } else {
            price
        }
    }

    /// How many effects the rule names: exactly one is valid.
    pub fn effects(&self) -> usize {
        [self.usd, self.per_second_usd, self.per_megapixel_usd, self.per_item_usd, self.multiply, self.multiply_per_megapixel_above, self.add_usd]
            .iter()
            .filter(|e| e.is_some())
            .count()
    }
}

/// What a request costs before it runs: its unit price × the outputs it asks
/// for — an image model's `n`, else one (a multiview call, a video, a mesh).
/// `params` is the validated request or the extras built from it, serialised.
pub fn request_price_usd(schema: &ModelSchema, base: &BaseGenerationRequest, params: &Value) -> f64 {
    let units = if schema.media_type == super::MediaType::Image && schema.task != super::ModelTask::Multiview {
        f64::from(base.n.max(1))
    } else {
        1.0
    };
    unit_price_usd(schema, &PriceFacts::new(schema, base, params)) * units
}

/// A request with no params of its own — a segmentation (PixelGenie L7) —
/// priced by its row alone: the quote, the reservation and the charge.
pub fn row_price_usd(schema: &ModelSchema) -> f64 {
    match serde_json::from_value::<BaseGenerationRequest>(serde_json::json!({ "model": schema.id })) {
        Ok(base) => unit_price_usd(schema, &PriceFacts::new(schema, &base, &Value::Null)),
        Err(_) => schema.pricing.base_cost_usd,
    }
}

/// The vendor's price for one output of this request.
pub fn unit_price_usd(schema: &ModelSchema, facts: &PriceFacts) -> f64 {
    schema
        .pricing
        .variable_pricing
        .iter()
        .filter(|rule| rule.applies(facts))
        .fold(schema.pricing.base_cost_usd, |price, rule| rule.apply(price, facts))
        .max(0.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn row(params: &str, rules: &str) -> ModelSchema {
        row_of("video", params, rules)
    }

    fn row_of(media: &str, params: &str, rules: &str) -> ModelSchema {
        let caps = if media == "image" { "text_to_image" } else { "text_to_video" };
        let yaml = format!(
            r#"
models:
  - id: x/m
    provider: x
    media_type: {media}
    display_name: M
    pricing:
      base_cost_usd: 0.5
      variable_pricing:
{rules}
    capabilities: {{ {caps}: true }}
    prompt: {{ required: true }}
    params:
{params}
    extra_allowlist: [addons]
"#
        );
        crate::capabilities::CapabilityRegistry::from_yaml_strs(&[("t.yaml", &yaml)]).unwrap().get("x/m").unwrap().clone()
    }

    fn base(extra: Value, refs: Value) -> BaseGenerationRequest {
        serde_json::from_value(json!({ "model": "x/m", "prompt": "p", "extra": extra, "reference_images": refs })).unwrap()
    }

    fn price(m: &ModelSchema, params: Value) -> f64 {
        unit_price_usd(m, &PriceFacts::new(m, &base(Value::Null, json!([])), &params))
    }

    const VIDEO_PARAMS: &str = "      duration_seconds: { kind: float, min: 1, max: 10, default: 5 }\n      resolution: { kind: string, enum_values: [720p, 1080p], default: 720p }";

    #[test]
    fn a_per_second_rate_follows_the_clip_and_its_resolution() {
        let m = row(VIDEO_PARAMS, "        - { per_second_usd: 0.1 }\n        - { when: { resolution: 1080p }, per_second_usd: 0.2 }");
        assert!((price(&m, json!({})) - 0.5).abs() < 1e-9, "the default 5 s at 720p");
        assert!((price(&m, json!({ "duration_seconds": 8.0 })) - 0.8).abs() < 1e-9);
        assert!((price(&m, json!({ "duration_seconds": 8.0, "resolution": "1080p" })) - 1.6).abs() < 1e-9);
    }

    #[test]
    fn values_multipliers_and_add_ons_apply_in_order() {
        let m = row(
            "      texture: { kind: bool, default: false }\n      pbr: { kind: bool, default: false }\n      target_polycount: { kind: int, min: 1, max: 9 }",
            "        - { when: { texture: true }, multiply: 3 }\n        - { when: { pbr: true }, add_usd: 0.15 }\n        - { when: { target_polycount: '*' }, add_usd: 0.15 }\n        - { when: { extra.addons: HighPack }, multiply: 2 }",
        );
        assert!((price(&m, json!({})) - 0.5).abs() < 1e-9);
        assert!((price(&m, json!({ "texture": true })) - 1.5).abs() < 1e-9);
        assert!((price(&m, json!({ "texture": true, "pbr": true, "target_polycount": 5 })) - 1.8).abs() < 1e-9);
        let extra = unit_price_usd(&m, &PriceFacts::new(&m, &base(json!({ "addons": "HighPack" }), json!([])), &json!({})));
        assert!((extra - 1.0).abs() < 1e-9, "{extra}");
    }

    #[test]
    fn a_per_megapixel_rate_reads_the_size_and_an_item_rate_counts_its_list() {
        let m = row_of("image", "      size: { kind: size, mode: enum, values: [[1024, 1024], [2048, 1024]] }", "        - { per_megapixel_usd: 0.04 }");
        assert!((price(&m, json!({})) - 0.04).abs() < 1e-9, "1 MP when no size is sent");
        assert!((price(&m, json!({ "size": "2048x1024" })) - 0.04 * 2.097152).abs() < 1e-9);
        let m = row("      {}", "        - { per_item_usd: 0.1, per_item: animations }");
        assert!((price(&m, json!({ "animations": [{ "name": "a" }, { "name": "b" }, { "name": "c" }] })) - 0.3).abs() < 1e-9);
        let m = row("      {}", "        - { when: { views: '*' }, add_usd: 0.15 }");
        let views = unit_price_usd(&m, &PriceFacts::new(&m, &base(Value::Null, json!([{ "type": "url", "value": "u", "role": "view-front" }])), &json!({})));
        assert!((views - 0.65).abs() < 1e-9, "{views}");
    }

    /// OpenAI bills a larger image by its output tokens, which grow with its
    /// pixels: the price scales past the anchor size, never below it.
    #[test]
    fn a_megapixel_multiplier_scales_an_image_larger_than_its_anchor() {
        let m = row_of(
            "image",
            "      size: { kind: size, mode: freeform, min_width: 16, max_width: 4096, min_height: 16, max_height: 4096 }",
            "        - { multiply_per_megapixel_above: 1.048576 }",
        );
        let at = |size: &str| price(&m, json!({ "size": size }));
        assert!((at("2048x2048") - 4.0 * at("1024x1024")).abs() < 1e-9, "{} vs {}", at("2048x2048"), at("1024x1024"));
        assert!((at("512x512") - at("1024x1024")).abs() < 1e-9, "never below the anchor");
        assert!((price(&m, json!({})) - at("1024x1024")).abs() < 1e-9, "no size is the anchor");
    }

    #[test]
    fn a_clip_made_of_keyframes_lasts_until_its_last_one() {
        let m = row("      {}", "        - { per_second_usd: 0.1 }");
        let refs = json!([{ "type": "url", "value": "a", "role": "keyframe", "time_s": 0.0 }, { "type": "url", "value": "b", "role": "keyframe", "time_s": 7.0 }]);
        let p = unit_price_usd(&m, &PriceFacts::new(&m, &base(Value::Null, refs), &json!({})));
        assert!((p - 0.7).abs() < 1e-9, "{p}");
    }
}
