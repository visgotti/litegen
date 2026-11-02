//! The rmcp `ServerHandler`: one instance per request, for one endpoint.

use std::sync::Arc;

use rmcp::model::{Implementation, ServerCapabilities, ServerConfig};
use rmcp::ServerHandler;

use super::{McpShared, Surface};

#[derive(Clone)]
pub struct LiteGenMcp {
    pub surface: Surface,
    pub shared: Arc<McpShared>,
}

impl ServerHandler for LiteGenMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(
                Implementation::new("litegen", env!("CARGO_PKG_VERSION")).with_title(self.surface.title()),
            )
            .with_instructions(self.surface.instructions())
    }
}
