//! The rmcp `ServerHandler`: one instance per request, for one endpoint.

use std::sync::Arc;

use rmcp::model::{CacheScope, Implementation, ListToolsResult, PaginatedRequestParams, ServerCapabilities, ServerConfig};
use rmcp::service::RequestContext;
use rmcp::{ErrorData as McpError, RoleServer, ServerHandler};

use super::auth::Caller;
use super::{McpShared, Surface};

/// How long a client may cache `tools/list`. The list depends on the caller's
/// credential, so only that caller's client may cache it.
const TOOLS_TTL_MS: u64 = 300_000;

#[derive(Clone)]
pub struct LiteGenMcp {
    pub surface: Surface,
    pub shared: Arc<McpShared>,
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
}
