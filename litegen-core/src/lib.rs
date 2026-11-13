// Handlers and their helpers return `Result<_, axum::response::Response>`, so
// every early return carries a 128-byte `Response`; boxing them all would be
// noise for no measurable gain.
#![allow(clippy::result_large_err)]

pub mod api;
pub mod auth;
pub mod capabilities;
pub mod access;
pub mod config;
pub mod hooks;
pub mod db;
pub mod management;
pub mod mcp;
// `mesh` is a standalone library that litegen work doesn't edit
// (.claude/rules/mesh-standalone.md), so its style lints are allowed here until
// it is its own crate with its own lint policy.
#[allow(
    clippy::chunks_exact_to_as_chunks,
    clippy::manual_is_multiple_of,
    clippy::needless_range_loop,
    clippy::neg_cmp_op_on_partial_ord,
    clippy::no_effect_replace,
    clippy::redundant_guards,
    clippy::type_complexity
)]
pub mod mesh;
pub mod observability;
pub mod providers;
pub mod proxy;
pub mod serve;
pub mod shared;
pub mod store;
pub mod supervise;
pub mod types;
pub mod util;

/// The engine names no server module (.claude/rules/engine-boundary.md).
#[cfg(test)]
mod engine_boundary_tests;

/// Return the path to the models directory.
/// Respects the `LITEGEN_MODELS_DIR` environment variable; defaults to `"models"`.
pub fn models_dir_path() -> std::path::PathBuf {
    std::path::PathBuf::from(
        std::env::var("LITEGEN_MODELS_DIR").unwrap_or_else(|_| "models".to_string())
    )
}

#[cfg(test)]
mod config_tests {
    use super::models_dir_path;

    /// One test, not two: both read and write the same process environment
    /// variable, and run in parallel they raced (the default read the other
    /// test's value).
    #[test]
    fn models_dir_path_defaults_to_models_and_respects_env_var() {
        std::env::remove_var("LITEGEN_MODELS_DIR");
        assert_eq!(models_dir_path().to_str().unwrap(), "models");

        std::env::set_var("LITEGEN_MODELS_DIR", "/custom/models/path");
        let path = models_dir_path();
        std::env::remove_var("LITEGEN_MODELS_DIR");
        assert_eq!(path.to_str().unwrap(), "/custom/models/path");
    }
}
