//! A tool call becomes an ordinary HTTP request to the API router, with the
//! caller's own credentials, so it passes through exactly the middleware and
//! handler checks the HTTP API runs: authentication, scopes, tenancy, budgets,
//! rate limits, idempotency and audit.

use std::collections::HashSet;

use axum::body::{Body, Bytes};
use axum::http::request::Builder;
use axum::http::{header, HeaderMap, Request, StatusCode};
use base64::Engine;
use rmcp::model::JsonObject;
use serde_json::{Map, Value};
use tower::ServiceExt;

use super::auth::Caller;
use super::registry::ToolSpec;
use super::schema::{ArgPlan, BodyPlan};
use super::McpShared;

/// A reply from the API router.
#[derive(Debug, Clone)]
pub struct HttpReply {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: Bytes,
}

/// Largest reply body a tool call reads.
const MAX_REPLY_BYTES: usize = 64 * 1024 * 1024;

/// The spend tag every call through MCP carries.
pub const MCP_TAG: &str = "mcp";

/// The request for a JSON (or bodiless) operation.
pub fn build_request(spec: &ToolSpec, plan: &ArgPlan, caller: &Caller, args: &JsonObject) -> Result<Request<Body>, String> {
    if plan.binary_in {
        return Err(format!("{} sends a file: give source_url or data_base64", spec.def.name));
    }
    let (mut builder, mut known) = head(spec, plan, caller, args)?;
    let body = match &plan.body {
        BodyPlan::Flattened(_) => {
            // Unknown arguments travel too: the API's validator decides what an
            // unexpected field means, exactly as for a direct HTTP call.
            let fields: Map<String, Value> = args.iter().filter(|(k, _)| !known.contains(k.as_str())).map(|(k, v)| (k.clone(), v.clone())).collect();
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(Value::Object(fields).to_string())
        }
        BodyPlan::Nested => {
            known.insert("body");
            reject_unknown(args, &known)?;
            builder = builder.header(header::CONTENT_TYPE, "application/json");
            Body::from(args.get("body").cloned().unwrap_or(Value::Object(Map::new())).to_string())
        }
        BodyPlan::None => {
            reject_unknown(args, &known)?;
            Body::empty()
        }
    };
    builder.body(body).map_err(|e| e.to_string())
}

/// The request for an operation that takes raw bytes (`convert_3d_model`).
pub fn build_upload(spec: &ToolSpec, plan: &ArgPlan, caller: &Caller, args: &JsonObject, bytes: Vec<u8>) -> Result<Request<Body>, String> {
    let (builder, mut known) = head(spec, plan, caller, args)?;
    known.extend(["source_url", "data_base64"]);
    reject_unknown(args, &known)?;
    builder.header(header::CONTENT_TYPE, "application/octet-stream").body(Body::from(bytes)).map_err(|e| e.to_string())
}

/// The file an upload tool sends: `data_base64` decoded, or `source_url`
/// fetched with the materializer's SSRF-guarded, size-capped client.
pub async fn upload_bytes(shared: &McpShared, args: &JsonObject) -> Result<Vec<u8>, String> {
    let data = args.get("data_base64").and_then(Value::as_str);
    let url = args.get("source_url").and_then(Value::as_str);
    match (data, url) {
        (Some(data), None) => base64::engine::general_purpose::STANDARD
            .decode(data.trim())
            .map_err(|e| format!("data_base64 is not valid base64: {e}")),
        (None, Some(url)) => shared.state.materializer.fetch_url(url).await.map_err(|e| format!("could not fetch source_url: {e}")),
        _ => Err("give exactly one of source_url and data_base64".to_string()),
    }
}

/// Method, URI and headers, plus the argument names they used.
fn head<'a>(spec: &ToolSpec, plan: &'a ArgPlan, caller: &Caller, args: &JsonObject) -> Result<(Builder, HashSet<&'a str>), String> {
    let def = spec.def;

    let mut path = def.path.to_string();
    for name in &plan.path {
        let value = args.get(name).filter(|v| !v.is_null()).ok_or_else(|| format!("missing required argument '{name}'"))?;
        let raw = scalar(value).ok_or_else(|| format!("argument '{name}' must be a string or a number"))?;
        let encoded = if def.catch_all == Some(name.as_str()) {
            // Model ids such as `fal/flux-pro` go to a catch-all route: keep the `/`.
            raw.split('/').map(|segment| urlencoding::encode(segment).into_owned()).collect::<Vec<_>>().join("/")
        } else {
            urlencoding::encode(&raw).into_owned()
        };
        path = path.replace(&format!("{{{name}}}"), &encoded);
    }

    let mut query = Vec::new();
    for name in &plan.query {
        let Some(value) = args.get(name).filter(|v| !v.is_null()) else { continue };
        let text = match value {
            Value::Array(items) => items.iter().filter_map(scalar).collect::<Vec<_>>().join(","),
            other => scalar(other).ok_or_else(|| format!("argument '{name}' must be a string, number or boolean"))?,
        };
        query.push(format!("{}={}", urlencoding::encode(name), urlencoding::encode(&text)));
    }
    let uri = if query.is_empty() { path } else { format!("{path}?{}", query.join("&")) };

    let mut builder = authorized(Request::builder().method(def.method.method()).uri(uri), caller);

    let mut tags = vec![MCP_TAG.to_string()];
    for (arg, header_name) in &plan.headers {
        let Some(value) = args.get(arg).filter(|v| !v.is_null()) else { continue };
        if *header_name == "x-litegen-tags" {
            let list: Vec<String> = match value {
                Value::Array(items) => items.iter().filter_map(scalar).collect(),
                other => scalar(other).into_iter().collect(),
            };
            tags.extend(list.into_iter().filter(|t| !t.is_empty() && t != MCP_TAG));
            continue;
        }
        let text = scalar(value).ok_or_else(|| format!("argument '{arg}' must be a string"))?;
        builder = builder.header(*header_name, text);
    }
    builder = builder.header("x-litegen-tags", tags.join(","));

    // Everything a tool argument can mean besides a body field.
    let mut known: HashSet<&str> = plan.path.iter().chain(&plan.query).chain(&plan.local).map(String::as_str).collect();
    known.extend(plan.headers.iter().map(|(arg, _)| arg.as_str()));
    Ok((builder, known))
}

/// A request builder carrying the caller's credentials.
pub fn authorized(builder: Builder, caller: &Caller) -> Builder {
    match &caller.authorization {
        Some(authorization) => builder.header(header::AUTHORIZATION, authorization.clone()),
        None => builder,
    }
}

fn reject_unknown(args: &JsonObject, known: &HashSet<&str>) -> Result<(), String> {
    match args.keys().find(|k| !known.contains(k.as_str())) {
        Some(unknown) => Err(format!("unknown argument '{unknown}'")),
        None => Ok(()),
    }
}

/// A JSON scalar as text, or `None` for objects, arrays and null.
fn scalar(value: &Value) -> Option<String> {
    match value {
        Value::String(s) => Some(s.clone()),
        Value::Number(n) => Some(n.to_string()),
        Value::Bool(b) => Some(b.to_string()),
        _ => None,
    }
}

/// GET an API path as the caller (previews of LiteGen-hosted images, asset
/// lookups).
pub async fn get(shared: &McpShared, caller: &Caller, path_and_query: &str) -> HttpReply {
    let req = authorized(Request::builder().method("GET").uri(path_and_query), caller).body(Body::empty());
    match req {
        Ok(req) => send(&shared.api, req).await,
        Err(_) => HttpReply { status: StatusCode::BAD_REQUEST, headers: HeaderMap::new(), body: Bytes::new() },
    }
}

/// Run a request through the API router and read its reply.
pub async fn send(api: &axum::Router, req: Request<Body>) -> HttpReply {
    let response = match api.clone().oneshot(req).await {
        Ok(response) => response,
        Err(never) => match never {},
    };
    let (parts, body) = response.into_parts();
    match axum::body::to_bytes(body, MAX_REPLY_BYTES).await {
        Ok(body) => HttpReply { status: parts.status, headers: parts.headers, body },
        Err(_) => HttpReply {
            status: StatusCode::BAD_GATEWAY,
            headers: HeaderMap::new(),
            body: Bytes::from_static(br#"{"error":{"message":"The reply was larger than MCP's 64 MiB limit; call the HTTP API directly.","type":"reply_too_large","code":"reply_too_large"}}"#),
        },
    }
}
