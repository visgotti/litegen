//! Helpers for the MCP integration suites: the full app with or without
//! authentication, and a small MCP client that speaks both the 2025-11-25
//! revision (`initialize` first) and the stateless 2026-07-28 revision over the
//! real route table.
#![allow(dead_code)]

use std::sync::Arc;

use axum::body::Body;
use axum::http::{HeaderMap, Request, StatusCode};
use axum::Router;
use litegen::config::{AppConfig, Mode};
use litegen::db::sqlite::SqliteDatabase;
use serde_json::{json, Value};
use tower::ServiceExt;

use crate::harness;

pub const LEGACY: &str = "2025-11-25";
pub const CURRENT: &str = "2026-07-28";
pub const MASTER_KEY: &str = "test-master";

/// Dev mode: single-tenant, no master key, so no credentials are needed.
pub async fn app(tweak: impl FnOnce(&mut AppConfig)) -> (Router, Arc<SqliteDatabase>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = harness::app_over_with(db.clone(), dir.path(), tweak, None, Mode::SingleTenant).await;
    (app, db, dir)
}

/// Authentication on: the master key `MASTER_KEY` and the given mode.
pub async fn secured_app(mode: Mode) -> (Router, Arc<SqliteDatabase>, tempfile::TempDir) {
    secured_app_with(mode, |_| {}).await
}

pub async fn secured_app_with(
    mode: Mode,
    tweak: impl FnOnce(&mut AppConfig),
) -> (Router, Arc<SqliteDatabase>, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
    let app = harness::app_over_with(db.clone(), dir.path(), tweak, Some(MASTER_KEY), mode).await;
    (app, db, dir)
}

/// A JSON-RPC exchange: the HTTP status, the response's `result` or `error`,
/// and any notifications that arrived on the same response stream.
#[derive(Debug)]
pub struct Rpc {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub result: Option<Value>,
    pub error: Option<Value>,
    pub notifications: Vec<Value>,
    pub body: String,
}

pub struct Mcp<'a> {
    pub app: &'a Router,
    pub path: String,
    pub host: String,
    pub bearer: Option<String>,
    pub protocol: &'static str,
    pub extra: Vec<(String, String)>,
}

impl<'a> Mcp<'a> {
    pub fn new(app: &'a Router, path: &str) -> Self {
        Mcp { app, path: path.to_string(), host: "localhost".into(), bearer: None, protocol: CURRENT, extra: Vec::new() }
    }

    pub fn host(mut self, host: &str) -> Self {
        self.host = host.to_string();
        self
    }

    pub fn bearer(mut self, token: &str) -> Self {
        self.bearer = Some(token.to_string());
        self
    }

    pub fn legacy(mut self) -> Self {
        self.protocol = LEGACY;
        self
    }

    pub fn header(mut self, name: &str, value: &str) -> Self {
        self.extra.push((name.to_string(), value.to_string()));
        self
    }

    /// POST one JSON-RPC request. On the current revision the request carries
    /// the per-request `_meta` protocol fields and the `Mcp-Method` /
    /// `Mcp-Name` headers, as 2026-07-28 clients send them.
    pub async fn rpc(&self, method: &str, params: Value) -> Rpc {
        let mut params = if params.is_null() { json!({}) } else { params };
        if self.protocol == CURRENT {
            let meta = params.as_object_mut().unwrap().entry("_meta").or_insert_with(|| json!({}));
            let meta = meta.as_object_mut().unwrap();
            meta.insert("io.modelcontextprotocol/protocolVersion".into(), json!(CURRENT));
            meta.insert("io.modelcontextprotocol/clientCapabilities".into(), json!({}));
            meta.insert("io.modelcontextprotocol/clientInfo".into(), json!({ "name": "litegen-tests", "version": "0" }));
        }
        let body = json!({ "jsonrpc": "2.0", "id": 1, "method": method, "params": params });
        let mut req = Request::builder()
            .method("POST")
            .uri(&self.path)
            .header("host", self.host.as_str())
            .header("content-type", "application/json")
            .header("accept", "application/json, text/event-stream")
            .header("mcp-protocol-version", self.protocol);
        if self.protocol == CURRENT {
            req = req.header("mcp-method", method);
            if method == "tools/call" {
                req = req.header("mcp-name", body["params"]["name"].as_str().unwrap_or_default());
            }
        }
        if let Some(token) = &self.bearer {
            req = req.header("authorization", format!("Bearer {token}"));
        }
        for (k, v) in &self.extra {
            req = req.header(k.as_str(), v.as_str());
        }
        let resp = self.app.clone().oneshot(req.body(Body::from(body.to_string())).unwrap()).await.unwrap();
        let status = resp.status();
        let headers = resp.headers().clone();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
        let text = String::from_utf8_lossy(&bytes).to_string();
        parse(status, headers, text)
    }

    /// The `result` of a request that must succeed at the JSON-RPC level.
    pub async fn ok(&self, method: &str, params: Value) -> Value {
        let rpc = self.rpc(method, params).await;
        assert_eq!(rpc.status, StatusCode::OK, "{method}: HTTP {} {}", rpc.status, rpc.body);
        rpc.result.unwrap_or_else(|| panic!("{method}: no result: {}", rpc.body))
    }

    /// Every tool name `tools/list` returns, following `nextCursor`.
    pub async fn tool_names(&self) -> Vec<String> {
        let mut names = Vec::new();
        let mut cursor: Option<String> = None;
        loop {
            let params = match &cursor {
                Some(c) => json!({ "cursor": c }),
                None => json!({}),
            };
            let result = self.ok("tools/list", params).await;
            for tool in result["tools"].as_array().unwrap() {
                names.push(tool["name"].as_str().unwrap().to_string());
            }
            match result["nextCursor"].as_str() {
                Some(next) => cursor = Some(next.to_string()),
                None => return names,
            }
        }
    }

    /// The `tools/list` entries themselves.
    pub async fn tools(&self) -> Vec<Value> {
        self.ok("tools/list", json!({})).await["tools"].as_array().unwrap().clone()
    }

    /// `tools/call`; returns the CallToolResult.
    pub async fn call(&self, tool: &str, arguments: Value) -> Value {
        self.ok("tools/call", json!({ "name": tool, "arguments": arguments })).await
    }
}

/// Read a JSON or SSE response body into its response and notifications.
fn parse(status: StatusCode, headers: HeaderMap, body: String) -> Rpc {
    let is_sse = headers
        .get("content-type")
        .and_then(|v| v.to_str().ok())
        .is_some_and(|ct| ct.starts_with("text/event-stream"));
    let messages: Vec<Value> = if is_sse {
        body.lines()
            .filter_map(|line| line.strip_prefix("data:"))
            .filter_map(|data| serde_json::from_str(data.trim()).ok())
            .collect()
    } else {
        serde_json::from_str(&body).map(|v| vec![v]).unwrap_or_default()
    };
    let mut rpc = Rpc { status, headers, result: None, error: None, notifications: Vec::new(), body };
    for message in messages {
        if message.get("id").is_some() && message.get("method").is_none() {
            rpc.result = message.get("result").cloned();
            rpc.error = message.get("error").cloned();
        } else if message.get("method").is_some() {
            rpc.notifications.push(message);
        }
    }
    rpc
}

/// Sign up a new user (hosted mode) and return their `litegen_session=…`
/// cookie: a real browser session.
pub async fn signup_cookie(app: &Router) -> String {
    let email = format!("user-{}@example.com", uuid::Uuid::new_v4());
    let body = json!({ "email": email, "password": "a-long-test-password-1", "org_name": "MCP Org" });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/auth/signup")
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK, "signup failed");
    resp.headers()
        .get_all("set-cookie")
        .iter()
        .filter_map(|v| v.to_str().ok())
        .filter_map(|s| s.split(';').next())
        .find(|kv| kv.starts_with("litegen_session="))
        .expect("session cookie")
        .to_string()
}

/// Create an API key with the master key; returns the raw secret.
pub async fn create_key(app: &Router, scopes: &str) -> String {
    create_key_with_id(app, scopes).await.1
}

/// GET an API path with a bearer token; returns the JSON body (asserting 200).
pub async fn http_get(app: &Router, path: &str, bearer: &str) -> Value {
    let req = Request::builder()
        .method("GET")
        .uri(path)
        .header("authorization", format!("Bearer {bearer}"))
        .body(Body::empty())
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 16 * 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap_or(Value::Null);
    assert_eq!(status, StatusCode::OK, "GET {path}: {json}");
    json
}

/// Create an API key with the master key; returns `(id, secret)`.
pub async fn create_key_with_id(app: &Router, scopes: &str) -> (String, String) {
    let body = json!({ "name": format!("mcp-{scopes}"), "scopes": scopes });
    let req = Request::builder()
        .method("POST")
        .uri("/v1/keys")
        .header("authorization", format!("Bearer {MASTER_KEY}"))
        .header("content-type", "application/json")
        .body(Body::from(body.to_string()))
        .unwrap();
    let resp = app.clone().oneshot(req).await.unwrap();
    let status = resp.status();
    let bytes = axum::body::to_bytes(resp.into_body(), 1024 * 1024).await.unwrap();
    let json: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(status, StatusCode::CREATED, "create key {scopes}: {json}");
    (json["id"].as_str().unwrap().to_string(), json["key"].as_str().unwrap().to_string())
}
