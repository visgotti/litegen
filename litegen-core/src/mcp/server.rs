//! The rmcp `ServerHandler`: one instance per request, for one endpoint.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use rmcp::model::{
    CacheScope, CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, JsonObject,
    ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig,
};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};
use tracing::Instrument;

use super::auth::Caller;
use super::registry::{SpecKind, ToolSpec};
use super::schema::ArgPlan;
use super::wait::{self, ProgressSink};
use super::{content, dispatch, gate, McpShared, Surface};

/// How long a client may cache `tools/list`. The list depends on the caller's
/// credential, so only that caller's client may cache it.
const TOOLS_TTL_MS: u64 = 300_000;
/// How often a long tool call reports that it is still working, when the
/// client sent a progress token (Claude Code's idle timer resets on these).
const HEARTBEAT: Duration = Duration::from_secs(10);

#[derive(Clone)]
pub struct LiteGenMcp {
    pub surface: Surface,
    pub shared: Arc<McpShared>,
}

impl LiteGenMcp {
    /// Replay one HTTP-backed tool call through the API router.
    async fn run_http(&self, spec: &ToolSpec, plan: &ArgPlan, caller: &Caller, args: &JsonObject) -> CallToolResult {
        let request = if plan.binary_in {
            match dispatch::upload_bytes(&self.shared, args).await {
                Ok(bytes) => dispatch::build_upload(spec, plan, caller, args, bytes),
                Err(message) => Err(message),
            }
        } else {
            dispatch::build_request(spec, plan, caller, args)
        };
        match request {
            Ok(req) => {
                let reply = dispatch::send(&self.shared.api, req).await;
                tracing::debug!(status = reply.status.as_u16(), "mcp tool replied");
                content::to_result(&self.shared, caller, spec, args, &reply).await
            }
            Err(message) => CallToolResult::error(vec![ContentBlock::text(message)]),
        }
    }
}

/// Run `work`, telling the client every ten seconds that it is still going.
async fn with_heartbeat<F: Future<Output = CallToolResult>>(sink: &ProgressSink, work: F) -> CallToolResult {
    tokio::pin!(work);
    let started = std::time::Instant::now();
    let mut ticker = tokio::time::interval_at(tokio::time::Instant::now() + HEARTBEAT, HEARTBEAT);
    loop {
        tokio::select! {
            result = &mut work => return result,
            _ = ticker.tick() => {
                let secs = started.elapsed().as_secs();
                sink.report(secs as f64, format!("Working… {secs}s")).await;
            }
        }
    }
}

/// The caller `auth::mcp_auth` put on the HTTP request.
fn caller(context: &RequestContext<RoleServer>) -> Result<Caller, McpError> {
    context
        .extensions
        .get::<axum::http::request::Parts>()
        .and_then(|parts| parts.extensions.get::<Caller>())
        .cloned()
        .ok_or_else(|| McpError::internal_error("request reached MCP without an authenticated caller", None))
}

impl ServerHandler for LiteGenMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("litegen", env!("CARGO_PKG_VERSION")).with_title(self.surface.title()),
            )
            .with_instructions(self.surface.instructions())
    }

    async fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        context: RequestContext<RoleServer>,
    ) -> Result<ListToolsResult, McpError> {
        let caller = caller(&context)?;
        let tools = self.shared.registry.visible(&caller, &self.shared.state).into_iter().map(|s| s.tool.clone()).collect();
        Ok(ListToolsResult::with_all_items(tools).with_ttl_ms(TOOLS_TTL_MS).with_cache_scope(CacheScope::Private))
    }

    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResponse, McpError> {
        let caller = caller(&context)?;
        let name = request.name.to_string();
        let args = request.arguments.unwrap_or_default();
        let surface = match self.surface {
            Surface::Ai => "ai",
            Surface::Admin => "admin",
        };
        let span = tracing::info_span!("mcp.tool", tool = %name, surface, caller = ?caller.kind);
        let progress = context.meta.get_progress_token().map(|token| ProgressSink::new(token, context.peer.clone()));
        async move {
            // A tool the caller cannot see does not exist for them.
            let spec = self.shared.registry.get(&name).filter(|s| gate::visible(s.def, &caller, &self.shared.state));
            let result = match spec {
                None => CallToolResult::error(vec![ContentBlock::text(format!("Unknown tool: {name}"))]),
                Some(spec) => match &spec.kind {
                    SpecKind::Wait => wait::run(&self.shared, &caller, spec, &args, progress.as_ref()).await,
                    SpecKind::Http(plan) => {
                        let work = self.run_http(spec, plan, &caller, &args);
                        match &progress {
                            Some(sink) => with_heartbeat(sink, work).await,
                            None => work.await,
                        }
                    }
                },
            };
            let outcome = if result.is_error == Some(true) { "error" } else { "ok" };
            metrics::counter!("litegen_mcp_tool_calls_total", "tool" => name.clone(), "surface" => surface, "outcome" => outcome).increment(1);
            Ok(result.into())
        }
        .instrument(span)
        .await
    }
}
