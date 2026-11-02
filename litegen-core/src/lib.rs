pub mod api;
pub mod auth;
pub mod capabilities;
pub mod config;
pub mod db;
pub mod management;
pub mod mcp;
pub mod mesh;
pub mod observability;
pub mod providers;
pub mod proxy;
pub mod types;
pub mod util;

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
