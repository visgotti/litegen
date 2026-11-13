pub mod cors;
#[cfg(test)]
mod error_text_scan_tests;
pub mod handlers;
pub mod metrics;
pub mod middleware;
pub mod openapi;
pub mod pagination;

pub use handlers::{create_router, create_router_with_hooks};
pub use metrics::init_prometheus;
