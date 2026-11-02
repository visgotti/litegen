//! The tools LiteGen offers, built once at startup: every catalog row turned
//! into an MCP `Tool` (schema from the OpenAPI document, cleaned description,
//! title and annotations), plus `wait_for_generation`.

use std::sync::Arc;

use rmcp::model::{Tool, ToolAnnotations};
use serde_json::{json, Map, Value};

use super::auth::Caller;
use super::catalog::{Gate, Http, Surfaces, ToolDef, TOOLS};
use super::gate;
use super::schema::{self, ArgPlan};
use super::Surface;
use crate::api::middleware::{AppState, Scope};

/// How a tool runs.
#[derive(Debug, Clone)]
pub enum SpecKind {
    /// Replayed as this HTTP request through the API router.
    Http(ArgPlan),
    /// `wait_for_generation`: polls a job's status route.
    Wait,
}

#[derive(Debug, Clone)]
pub struct ToolSpec {
    pub def: &'static ToolDef,
    pub kind: SpecKind,
    pub tool: Tool,
}

pub struct Registry {
    specs: Vec<ToolSpec>,
}

/// `wait_for_generation`: not an API operation, so not in the catalog.
pub static WAIT_FOR_GENERATION: ToolDef = ToolDef {
    name: "wait_for_generation",
    method: Http::Get,
    path: "",
    surfaces: Surfaces::Ai,
    toolset: "generations",
    gate: Gate::AnyScope(&[Scope::Generate, Scope::Read]),
    title: "Wait for generation",
    description: Some(
        "Wait for a video, 3D or other generation to finish, up to timeout_seconds (default 45, max 230). Returns the final result, or the current progress if it is still running; then call it again.",
    ),
    catch_all: None,
};

/// Default and ceiling for `wait_for_generation`'s `timeout_seconds`: under
/// Claude Code's 60 s request limit and claude.ai's 240 s call limit.
pub const WAIT_DEFAULT_SECS: u64 = 45;
pub const WAIT_MAX_SECS: u64 = 230;

const READ_ONLY_EXTRA: [&str; 4] = ["estimate_image_cost", "estimate_video_cost", "estimate_3d_model_cost", "wait_for_generation"];
const DESTRUCTIVE_PREFIXES: [&str; 8] = ["revoke_", "delete_", "block_", "remove_", "reset_", "rotate_", "transfer_", "clear_"];
const OPEN_WORLD: [&str; 5] = ["generate_image", "segment_image", "generate_video", "generate_3d_model", "wait_for_generation"];

impl Registry {
    /// Every tool, or the name of the first one whose schema cannot be built.
    pub fn build(doc: &Value) -> Result<Registry, String> {
        let mut specs = Vec::with_capacity(TOOLS.len() + 1);
        for def in TOOLS {
            let (mut input, plan) = schema::build(doc, def).map_err(|e| format!("MCP tool {}: {e}", def.name))?;
            clean_descriptions(&mut input);
            let description = match def.description {
                Some(d) => d.to_string(),
                None => openapi_description(doc, def).unwrap_or_else(|| def.title.to_string()),
            };
            specs.push(ToolSpec { def, kind: SpecKind::Http(plan), tool: tool(def, description, input) });
        }
        let wait = &WAIT_FOR_GENERATION;
        specs.push(ToolSpec {
            def: wait,
            kind: SpecKind::Wait,
            tool: tool(wait, wait.description.unwrap_or_default().to_string(), wait_schema()),
        });
        Ok(Registry { specs })
    }

    pub fn get(&self, name: &str) -> Option<&ToolSpec> {
        self.specs.iter().find(|s| s.def.name == name)
    }

    /// The tools `caller` can call, in catalog order.
    pub fn visible(&self, caller: &Caller, state: &AppState) -> Vec<&ToolSpec> {
        self.specs.iter().filter(|s| gate::visible(s.def, caller, state)).collect()
    }

    pub fn specs(&self) -> &[ToolSpec] {
        &self.specs
    }
}

fn tool(def: &ToolDef, description: String, input: Map<String, Value>) -> Tool {
    let read_only = def.method == Http::Get || READ_ONLY_EXTRA.contains(&def.name);
    let destructive = !read_only && (def.method == Http::Delete || DESTRUCTIVE_PREFIXES.iter().any(|p| def.name.starts_with(p)));
    let idempotent = matches!(def.method, Http::Get | Http::Put | Http::Delete);
    let annotations = ToolAnnotations::with_title(def.title)
        .read_only(read_only)
        .destructive(destructive)
        .idempotent(idempotent)
        .open_world(OPEN_WORLD.contains(&def.name));
    Tool::new(def.name, description, Arc::new(input)).with_title(def.title).with_annotations(annotations)
}

fn wait_schema() -> Map<String, Value> {
    let schema = json!({
        "type": "object",
        "properties": {
            "id": {
                "type": "string",
                "description": "The job id generate_video, generate_3d_model or another generation returned."
            },
            "timeout_seconds": {
                "type": "integer",
                "minimum": 1,
                "maximum": WAIT_MAX_SECS,
                "default": WAIT_DEFAULT_SECS,
                "description": "How long to wait before returning the job's current state."
            }
        },
        "required": ["id"]
    });
    schema.as_object().cloned().unwrap_or_default()
}

/// The operation's OpenAPI summary and description, cleaned for clients.
fn openapi_description(doc: &Value, def: &ToolDef) -> Option<String> {
    let pointer = format!("/paths/{}/{}", def.path.replace('~', "~0").replace('/', "~1"), def.method.as_str().to_ascii_lowercase());
    let op = doc.pointer(&pointer)?;
    let summary = op.get("summary").and_then(Value::as_str).unwrap_or("").trim();
    let description = op.get("description").and_then(Value::as_str).unwrap_or("").trim();
    let text = if description.is_empty() || summary.is_empty() || description.starts_with(summary) {
        description.to_string()
    } else {
        format!("{summary}\n\n{description}")
    };
    let text = if text.is_empty() { summary.to_string() } else { text };
    let cleaned = clean(&text);
    (!cleaned.is_empty()).then_some(cleaned)
}

const MAX_DESCRIPTION: usize = 1_200;

/// Strip what only LiteGen's own developers need: `Live:` curl examples,
/// code fences, and references to internal plan documents such as `(L14)` or
/// `(3D doc §4)`. Cut to 1,200 characters at a word boundary.
pub fn clean(text: &str) -> String {
    let mut lines: Vec<String> = Vec::new();
    let mut in_fence = false;
    for (i, line) in text.lines().enumerate() {
        let trimmed = line.trim();
        if trimmed.starts_with("```") {
            in_fence = !in_fence;
            continue;
        }
        if in_fence || trimmed.starts_with("Live:") {
            continue;
        }
        // Handler doc comments open with "GET /v1/… — …"; clients see the
        // operation elsewhere, so keep only the words after it.
        let line = if i == 0 { strip_route_prefix(trimmed) } else { line.trim_end() };
        lines.push(line.to_string());
    }
    let text = strip_internal_refs(&lines.join("\n"));
    truncate(&unwrap_paragraphs(&text), MAX_DESCRIPTION)
}

/// `GET /v1/x — Does y.` → `Does y.`; a line that is only a route → empty.
fn strip_route_prefix(line: &str) -> &str {
    let mut words = line.splitn(2, ' ');
    let (Some(method), Some(rest)) = (words.next(), words.next()) else { return line };
    if !["GET", "POST", "PUT", "PATCH", "DELETE"].contains(&method) || !rest.starts_with('/') {
        return line;
    }
    for dash in [" — ", " - ", ": "] {
        if let Some((_, after)) = rest.split_once(dash) {
            return after.trim();
        }
    }
    if rest.contains(' ') { line } else { "" }
}

/// Join hard-wrapped lines into paragraphs, keeping list items and blank-line
/// breaks.
fn unwrap_paragraphs(text: &str) -> String {
    let mut paragraphs: Vec<String> = Vec::new();
    for block in text.split("\n\n") {
        let mut joined = String::new();
        for line in block.lines().map(str::trim).filter(|l| !l.is_empty()) {
            let item = line.starts_with("- ") || line.starts_with("* ") || line.split_once(". ").is_some_and(|(n, _)| n.chars().all(|c| c.is_ascii_digit()));
            if !joined.is_empty() {
                joined.push(if item { '\n' } else { ' ' });
            }
            joined.push_str(line);
        }
        if !joined.is_empty() {
            paragraphs.push(joined);
        }
    }
    paragraphs.join("\n\n")
}

/// Remove parenthesised references to internal documents: `(L14)`,
/// `(PixelGenie L16)`, `(3D doc §4)`, `(3D doc §5.2)`.
fn strip_internal_refs(text: &str) -> String {
    let mut out = String::with_capacity(text.len());
    let mut rest = text;
    while let Some(open) = rest.find('(') {
        let Some(close_rel) = rest[open..].find(')') else { break };
        let inner = &rest[open + 1..open + close_rel];
        if is_internal_ref(inner) {
            out.push_str(rest[..open].trim_end_matches(' '));
            rest = &rest[open + close_rel + 1..];
        } else {
            out.push_str(&rest[..open + 1]);
            rest = &rest[open + 1..];
        }
    }
    out.push_str(rest);
    out
}

fn is_internal_ref(inner: &str) -> bool {
    let words: Vec<&str> = inner.split_whitespace().collect();
    let is_l_ref = |w: &str| w.len() > 1 && w.starts_with('L') && w[1..].chars().all(|c| c.is_ascii_digit());
    match words.as_slice() {
        [w] => is_l_ref(w),
        [_, w] => is_l_ref(w),
        [_, doc, section] => *doc == "doc" && section.starts_with('§'),
        _ => false,
    }
}

fn truncate(text: &str, max: usize) -> String {
    if text.chars().count() <= max {
        return text.to_string();
    }
    let cut: String = text.chars().take(max - 1).collect();
    let cut = cut.rsplit_once(' ').map(|(head, _)| head.to_string()).unwrap_or(cut);
    format!("{}…", cut.trim_end())
}

// ─── The tool reference (docs/mcp/tools.md) ───────────────────────────────────

/// The Markdown tool reference: per endpoint and toolset a table of tools,
/// then each tool's description and arguments. `examples/mcp_tools_doc.rs`
/// prints it, and a test keeps the committed file current.
pub fn render_reference(registry: &Registry) -> String {
    use std::fmt::Write;
    let mut out = String::new();
    let _ = writeln!(out, "# LiteGen MCP tool reference\n");
    let _ = writeln!(
        out,
        "Generated from LiteGen's tool registry by `cargo run --example mcp_tools_doc` (run in `litegen-core/`). A test fails when this file is out of date, so don't edit it by hand. Connecting: [README](README.md). What each requirement means: [permissions](permissions.md).\n"
    );
    for surface in [Surface::Ai, Surface::Admin] {
        let tools: Vec<&ToolSpec> = registry.specs.iter().filter(|s| s.def.surfaces.includes(surface)).collect();
        let what = match surface {
            Surface::Ai => "the AI API",
            Surface::Admin => "the management API",
        };
        let _ = writeln!(out, "## `{}`: {what} ({} tools)\n", surface.path(), tools.len());
        for toolset in surface.toolsets() {
            let in_set: Vec<&&ToolSpec> = tools.iter().filter(|s| s.def.toolset == *toolset).collect();
            if in_set.is_empty() {
                continue;
            }
            let _ = writeln!(out, "### Toolset `{toolset}`\n");
            let _ = writeln!(out, "| Tool | What it does | API operation | Needs | Read-only | Destructive |");
            let _ = writeln!(out, "|---|---|---|---|---|---|");
            for spec in in_set {
                let hints = spec.tool.annotations.as_ref();
                let yes = |b: Option<bool>| if b == Some(true) { "yes" } else { "no" };
                let _ = writeln!(
                    out,
                    "| [`{name}`](#{name}) | {title} | {op} | {needs} | {ro} | {de} |",
                    name = spec.def.name,
                    title = cell(spec.def.title),
                    op = operation(spec.def),
                    needs = cell(&gate_words(spec.def)),
                    ro = yes(hints.and_then(|a| a.read_only_hint)),
                    de = yes(hints.and_then(|a| a.destructive_hint)),
                );
            }
            out.push('\n');
        }
    }

    let _ = writeln!(out, "## Tools\n");
    for spec in &registry.specs {
        let def = spec.def;
        let _ = writeln!(out, "### `{}`\n", def.name);
        if let Some(description) = spec.tool.description.as_deref() {
            let _ = writeln!(out, "{}\n", description.trim());
        }
        let endpoints = match def.surfaces {
            Surfaces::Ai => "`/mcp`",
            Surfaces::Admin => "`/admin/mcp`",
            Surfaces::Both => "`/mcp` and `/admin/mcp`",
        };
        let _ = writeln!(out, "{} · on {endpoints} · needs {}\n", operation(def), gate_words(def));
        let properties = spec.tool.input_schema.get("properties").and_then(Value::as_object);
        let required: Vec<&str> = spec
            .tool
            .input_schema
            .get("required")
            .and_then(Value::as_array)
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .collect();
        match properties.filter(|p| !p.is_empty()) {
            None => {
                let _ = writeln!(out, "No arguments.\n");
            }
            Some(properties) => {
                let _ = writeln!(out, "| Argument | Type | Required | Description |");
                let _ = writeln!(out, "|---|---|---|---|");
                let mut names: Vec<&String> = properties.keys().collect();
                names.sort_by_key(|n| (!required.contains(&n.as_str()), n.as_str()));
                for name in names {
                    let schema = &properties[name.as_str()];
                    let _ = writeln!(
                        out,
                        "| `{name}` | {} | {} | {} |",
                        cell(&type_words(schema)),
                        if required.contains(&name.as_str()) { "yes" } else { "no" },
                        cell(&first_sentence(schema.get("description").and_then(Value::as_str).unwrap_or(""))),
                    );
                }
                out.push('\n');
            }
        }
    }
    out
}

fn operation(def: &ToolDef) -> String {
    if def.path.is_empty() {
        "polls the job's status route".to_string()
    } else {
        format!("`{} {}`", def.method.as_str(), def.path)
    }
}

/// A tool's requirement in words (see docs/mcp/permissions.md).
fn gate_words(def: &ToolDef) -> String {
    let scope = |s: &Scope| match s {
        Scope::Generate => "`generate`",
        Scope::Read => "`read`",
        Scope::Admin => "`admin`",
    };
    match def.gate {
        Gate::Scope(s) => format!("the {} scope", scope(&s)),
        Gate::AnyScope(scopes) => format!("the {} scope", scopes.iter().map(scope).collect::<Vec<_>>().join(" or ")),
        Gate::OrgRead(p) => format!("org read access (`read` or `admin` scope, or a member role with `{}`)", p.as_str()),
        Gate::OrgWrite(p) => format!("org write access (`admin` scope, or a member role with `{}`)", p.as_str()),
        Gate::Member(p) => format!("a signed-in member with `{}`, or the platform admin", p.as_str()),
        Gate::Perm(p) => format!("a signed-in user with `{}`", p.as_str()),
        Gate::Global(p) => format!("the platform admin (single-tenant: a user with `{}`)", p.as_str()),
        Gate::GatewayAdmin => "the platform admin (single-tenant: owners, admins and `admin` keys)".to_string(),
        Gate::User => "a signed-in user".to_string(),
        Gate::Any => "any credential".to_string(),
    }
}

fn type_words(schema: &Value) -> String {
    if let Some(values) = schema.get("enum").and_then(Value::as_array) {
        let shown: Vec<String> = values.iter().take(8).map(|v| format!("`{}`", v.as_str().map(str::to_string).unwrap_or_else(|| v.to_string()))).collect();
        let more = if values.len() > 8 { ", …" } else { "" };
        return format!("one of {}{more}", shown.join(", "));
    }
    match schema.get("type") {
        Some(Value::String(t)) if t == "array" => match schema.pointer("/items/type").and_then(Value::as_str) {
            Some(item) => format!("array of {item}"),
            None => "array".to_string(),
        },
        Some(Value::String(t)) => t.clone(),
        Some(Value::Array(types)) => types.iter().filter_map(Value::as_str).collect::<Vec<_>>().join(" or "),
        _ if schema.get("oneOf").is_some() || schema.get("anyOf").is_some() => "one of several shapes".to_string(),
        _ => "any".to_string(),
    }
}

fn first_sentence(text: &str) -> String {
    let line = text.trim().lines().next().unwrap_or("").trim();
    let sentence = match line.find(". ") {
        Some(i) => &line[..=i],
        None => line,
    };
    truncate(sentence, 200)
}

/// Text safe inside a Markdown table cell.
fn cell(text: &str) -> String {
    text.replace('|', "\\|").replace('\n', " ")
}

/// Clean every `description` inside a schema, recursively.
fn clean_descriptions(schema: &mut Map<String, Value>) {
    for (key, value) in schema.iter_mut() {
        match value {
            Value::String(text) if key == "description" => *text = clean(text),
            Value::Object(inner) => clean_descriptions(inner),
            Value::Array(items) => {
                for item in items {
                    if let Value::Object(inner) = item {
                        clean_descriptions(inner);
                    }
                }
            }
            _ => {}
        }
    }
}
