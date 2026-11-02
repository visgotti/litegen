//! Tool input schemas, derived from the in-process OpenAPI document (the one
//! `/openapi.json` serves), so a tool's arguments are its operation's path,
//! query and header parameters plus its JSON body, flattened into one object.
//!
//! Clients are strict about what they accept: Claude Code drops a tool whose
//! schema is not valid JSON Schema 2020-12 or has a root `allOf`/`oneOf`, and
//! several clients cannot follow `$ref`. So every reference is inlined,
//! `allOf` is merged, and OpenAPI's nullable unions collapse to the plain type
//! (an optional argument is simply left out).

use serde_json::{json, Map, Value};

use super::catalog::ToolDef;

/// How a tool's arguments become an HTTP request.
#[derive(Debug, Clone, Default)]
pub struct ArgPlan {
    /// Path parameters, in the order the path names them.
    pub path: Vec<String>,
    pub query: Vec<String>,
    pub body: BodyPlan,
    /// (argument, header) pairs sent as request headers.
    pub headers: Vec<(String, &'static str)>,
    /// MCP's own arguments, never sent to the API (`preview`).
    pub local: Vec<String>,
    /// The operation takes raw bytes: arguments `source_url` / `data_base64`.
    pub binary_in: bool,
    /// The operation answers with bytes rather than JSON.
    pub binary_out: bool,
}

#[derive(Debug, Clone, Default, PartialEq)]
pub enum BodyPlan {
    #[default]
    None,
    /// The JSON body's properties are top-level arguments.
    Flattened(Vec<String>),
    /// The JSON body is the `body` argument.
    Nested,
}

/// The billed tools: they also take the end-user and spend-tag headers.
const BILLED: [&str; 4] = ["generate_image", "segment_image", "generate_video", "generate_3d_model"];
/// The tools that can return image previews.
const PREVIEWS: [&str; 2] = ["generate_image", "get_generation_asset"];

pub fn build(doc: &Value, def: &ToolDef) -> Result<(Map<String, Value>, ArgPlan), String> {
    let method = def.method.as_str().to_ascii_lowercase();
    let op = doc
        .pointer(&format!("/paths/{}/{method}", def.path.replace('~', "~0").replace('/', "~1")))
        .ok_or_else(|| format!("{} {} is not in the OpenAPI document", def.method.as_str(), def.path))?;
    let schemas = doc.pointer("/components/schemas").cloned().unwrap_or(json!({}));

    let mut props = Map::new();
    let mut required: Vec<String> = Vec::new();
    let mut plan = ArgPlan::default();

    for param in op.get("parameters").and_then(Value::as_array).into_iter().flatten() {
        let name = param.get("name").and_then(Value::as_str).ok_or("parameter without a name")?;
        let location = param.get("in").and_then(Value::as_str).unwrap_or("");
        let mut schema = normalize(param.get("schema").unwrap_or(&json!({})), &schemas, &mut Vec::new());
        if let Some(description) = param.get("description").and_then(Value::as_str) {
            schema.insert("description".into(), json!(description));
        }
        let arg = match location {
            "path" => {
                if def.catch_all == Some(name) {
                    let before = schema.get("description").and_then(Value::as_str).unwrap_or("").trim().to_string();
                    let note = "May contain '/' (send it unescaped).";
                    let text = if before.is_empty() { note.replace("May", "may") } else { format!("{before} {}", note.replace("May", "may")) };
                    schema.insert("description".into(), json!(text));
                }
                plan.path.push(name.to_string());
                required.push(name.to_string());
                name.to_string()
            }
            "query" => {
                plan.query.push(name.to_string());
                if param.get("required").and_then(Value::as_bool).unwrap_or(false) {
                    required.push(name.to_string());
                }
                name.to_string()
            }
            "header" => {
                let (arg, header) = match name.to_ascii_lowercase().as_str() {
                    "idempotency-key" => ("idempotency_key", "idempotency-key"),
                    _ => return Err(format!("{}: unsupported header parameter {name}", def.name)),
                };
                if !schema.contains_key("description") {
                    schema.insert("description".into(), json!("Send the same key again on a retry, so the call is never billed twice."));
                }
                plan.headers.push((arg.to_string(), header));
                arg.to_string()
            }
            other => return Err(format!("{}: unsupported parameter location {other}", def.name)),
        };
        props.insert(arg, Value::Object(schema));
    }

    if let Some(content) = op.pointer("/requestBody/content").and_then(Value::as_object) {
        let body_required = op.pointer("/requestBody/required").and_then(Value::as_bool).unwrap_or(false);
        if let Some(json_body) = content.get("application/json") {
            let body = normalize(json_body.get("schema").unwrap_or(&json!({})), &schemas, &mut Vec::new());
            let fields = body.get("properties").and_then(Value::as_object).cloned();
            let collides = fields.as_ref().is_some_and(|f| f.keys().any(|k| props.contains_key(k)));
            match fields {
                Some(fields) if !collides => {
                    plan.body = BodyPlan::Flattened(fields.keys().cloned().collect());
                    for name in body.get("required").and_then(Value::as_array).into_iter().flatten().filter_map(Value::as_str) {
                        required.push(name.to_string());
                    }
                    props.extend(fields);
                }
                _ => {
                    plan.body = BodyPlan::Nested;
                    if body_required {
                        required.push("body".into());
                    }
                    props.insert("body".into(), Value::Object(body));
                }
            }
        } else if content.contains_key("application/octet-stream") {
            plan.binary_in = true;
            props.insert(
                "source_url".into(),
                json!({ "type": "string", "format": "uri", "description": "URL LiteGen downloads the file from. Give exactly one of source_url and data_base64." }),
            );
            props.insert(
                "data_base64".into(),
                json!({ "type": "string", "description": "The file itself, base64-encoded. Give exactly one of source_url and data_base64." }),
            );
        }
    }

    plan.binary_out = op
        .get("responses")
        .and_then(Value::as_object)
        .and_then(|r| r.iter().find(|(code, _)| code.starts_with('2')))
        .and_then(|(_, resp)| resp.get("content").and_then(Value::as_object))
        .is_some_and(|content| !content.is_empty() && !content.keys().any(|ct| ct.contains("json")));

    if BILLED.contains(&def.name) {
        props.insert(
            "end_user".into(),
            json!({ "type": "string", "description": "Your own customer's id, so spend and budgets can be tracked per customer (sent as x-litegen-end-user)." }),
        );
        props.insert(
            "tags".into(),
            json!({ "type": "array", "items": { "type": "string" }, "description": "Spend tags for reporting (sent as x-litegen-tags). Calls through MCP are also tagged mcp." }),
        );
        plan.headers.push(("end_user".into(), "x-litegen-end-user"));
        plan.headers.push(("tags".into(), "x-litegen-tags"));
    }
    if PREVIEWS.contains(&def.name) {
        props.insert(
            "preview".into(),
            json!({ "type": "boolean", "default": true, "description": "Also return small JPEG previews of the images (longest side 512 px, at most 2). Set false to skip them." }),
        );
        plan.local.push("preview".into());
    }

    let mut schema = Map::new();
    schema.insert("type".into(), json!("object"));
    schema.insert("properties".into(), Value::Object(props));
    let mut seen = std::collections::HashSet::new();
    required.retain(|r| seen.insert(r.clone()));
    if !required.is_empty() {
        schema.insert("required".into(), json!(required));
    }
    Ok((schema, plan))
}

/// Keywords that are OpenAPI vocabulary rather than JSON Schema, or that no
/// client needs, dropped from every schema.
const DROPPED: [&str; 7] = ["$schema", "contentMediaType", "contentEncoding", "discriminator", "xml", "externalDocs", "nullable"];

/// An inlined, client-friendly copy of `schema`. `seen` holds the names of the
/// components being expanded, so a recursive type ends as a plain object.
fn normalize(schema: &Value, components: &Value, seen: &mut Vec<String>) -> Map<String, Value> {
    let Some(obj) = schema.as_object() else { return Map::new() };
    let mut out = Map::new();

    if let Some(reference) = obj.get("$ref").and_then(Value::as_str) {
        let name = reference.rsplit('/').next().unwrap_or(reference).to_string();
        let target = components.get(&name).cloned().unwrap_or(json!({}));
        if seen.contains(&name) {
            out.insert("type".into(), json!("object"));
        } else {
            seen.push(name);
            out = normalize(&target, components, seen);
            seen.pop();
        }
    }

    if let Some(members) = obj.get("allOf").and_then(Value::as_array) {
        for member in members {
            merge(&mut out, normalize(member, components, seen));
        }
    }

    for keyword in ["oneOf", "anyOf"] {
        if let Some(members) = obj.get(keyword).and_then(Value::as_array) {
            // Drop the null member before normalizing: normalized, `{"type":"null"}`
            // loses its type and becomes `{}`, which matches every value.
            let kept: Vec<Map<String, Value>> = members
                .iter()
                .filter(|m| !is_null_schema(m))
                .map(|m| normalize(m, components, seen))
                .collect();
            match kept.len() {
                0 => {}
                1 => merge(&mut out, kept.into_iter().next().unwrap()),
                _ => {
                    out.insert(keyword.into(), Value::Array(kept.into_iter().map(Value::Object).collect()));
                }
            }
        }
    }

    for (key, value) in obj {
        match key.as_str() {
            "$ref" | "allOf" | "oneOf" | "anyOf" => {}
            k if DROPPED.contains(&k) => {}
            "format" if value == "binary" => {}
            "type" => {
                if let Some(t) = collapse_type(value) {
                    out.insert("type".into(), t);
                }
            }
            "properties" => {
                let props: Map<String, Value> = value
                    .as_object()
                    .into_iter()
                    .flatten()
                    .map(|(k, v)| (k.clone(), Value::Object(normalize(v, components, seen))))
                    .collect();
                merge(&mut out, Map::from_iter([("properties".to_string(), Value::Object(props))]));
            }
            "required" => merge(&mut out, Map::from_iter([("required".to_string(), value.clone())])),
            "items" | "additionalProperties" | "not" if value.is_object() => {
                out.insert(key.clone(), Value::Object(normalize(value, components, seen)));
            }
            "prefixItems" => {
                let items: Vec<Value> = value.as_array().into_iter().flatten().map(|v| Value::Object(normalize(v, components, seen))).collect();
                out.insert(key.clone(), Value::Array(items));
            }
            _ => {
                out.insert(key.clone(), value.clone());
            }
        }
    }
    out
}

/// `{"type": "null"}`: the null member of OpenAPI's nullable union.
fn is_null_schema(schema: &Value) -> bool {
    schema.get("type").is_some_and(|t| collapse_type(t).is_none())
}

/// `["string","null"]` → `"string"`; a lone type stays; several stay a list.
fn collapse_type(value: &Value) -> Option<Value> {
    match value {
        Value::Array(types) => {
            let kept: Vec<Value> = types.iter().filter(|t| *t != "null").cloned().collect();
            match kept.len() {
                0 => None,
                1 => kept.into_iter().next(),
                _ => Some(Value::Array(kept)),
            }
        }
        Value::String(t) if t == "null" => None,
        other => Some(other.clone()),
    }
}

/// Merge `from` into `into`: properties and required lists union; for any
/// other keyword the first value wins.
fn merge(into: &mut Map<String, Value>, from: Map<String, Value>) {
    for (key, value) in from {
        match (key.as_str(), into.get_mut(&key)) {
            ("properties", Some(Value::Object(existing))) => {
                if let Value::Object(more) = value {
                    existing.extend(more);
                }
            }
            ("required", Some(Value::Array(existing))) => {
                for item in value.as_array().into_iter().flatten() {
                    if !existing.contains(item) {
                        existing.push(item.clone());
                    }
                }
            }
            (_, Some(_)) => {}
            (_, None) => {
                into.insert(key, value);
            }
        }
    }
}
