//! LiteGen as the OAuth 2.1 authorization server for its two MCP endpoints,
//! following the MCP authorization spec: discovery documents, client
//! registration, a dashboard consent step, and opaque tokens that each work
//! on one endpoint only. ChatGPT and most claude.ai users cannot send an API
//! key, so for them this is the only way in.

pub mod sql;
pub mod store;
#[cfg(test)]
mod store_tests;
