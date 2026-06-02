pub mod api;
pub mod auth;
pub mod capabilities;
pub mod config;
pub mod db;
pub mod observability;
pub mod providers;
pub mod proxy;
pub mod types;

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

    #[test]
    fn models_dir_path_defaults_to_models() {
        // Only check the default when the var is not set
        std::env::remove_var("LITEGEN_MODELS_DIR");
        let path = models_dir_path();
        assert_eq!(path.to_str().unwrap(), "models");
    }

    #[test]
    fn models_dir_path_respects_env_var() {
        // Temporarily set the env var
        std::env::set_var("LITEGEN_MODELS_DIR", "/custom/models/path");
        let path = models_dir_path();
        std::env::remove_var("LITEGEN_MODELS_DIR");
        assert_eq!(path.to_str().unwrap(), "/custom/models/path");
    }
}
