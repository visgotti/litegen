/// OAuth provider configuration.  Populated from environment variables.
/// All OAuth logic is hand-rolled over `reqwest` — no oauth2 crate machinery
/// needed for the simple authorization-code flow used here.

#[derive(Clone, Debug, Default)]
pub struct OAuthConfig {
    pub github: Option<ProviderConfig>,
    pub google: Option<ProviderConfig>,
    /// Base URL used to build the redirect_uri for callbacks.
    /// E.g. `https://app.example.com` → callbacks at `{base}/v1/auth/oauth/github/callback`.
    pub callback_base: Option<String>,

    // ── Test overrides ─────────────────────────────────────────────────────
    // None = use real GitHub/Google endpoints. Set in tests to point at wiremock.

    /// Override for GitHub authorize base (default: `https://github.com`).
    pub github_authorize_base: Option<String>,
    /// Override for GitHub API base (default: `https://api.github.com`).
    pub github_api_base: Option<String>,
    /// Override for GitHub token exchange base (default: `https://github.com`).
    pub github_token_base: Option<String>,

    /// Override for Google authorize base (default: `https://accounts.google.com`).
    pub google_authorize_base: Option<String>,
    /// Override for Google token endpoint base (default: `https://oauth2.googleapis.com`).
    pub google_token_base: Option<String>,
    /// Override for Google userinfo endpoint base (default: `https://openidconnect.googleapis.com`).
    pub google_userinfo_base: Option<String>,
}

#[derive(Clone, Debug)]
pub struct ProviderConfig {
    pub client_id: String,
    pub client_secret: String,
}

impl OAuthConfig {
    pub fn from_env() -> Self {
        Self {
            github: env_pair(
                "LITEGEN__OAUTH__GITHUB__CLIENT_ID",
                "LITEGEN__OAUTH__GITHUB__CLIENT_SECRET",
            ),
            google: env_pair(
                "LITEGEN__OAUTH__GOOGLE__CLIENT_ID",
                "LITEGEN__OAUTH__GOOGLE__CLIENT_SECRET",
            ),
            callback_base: std::env::var("LITEGEN__OAUTH__CALLBACK_BASE").ok(),
            // Test URL overrides — always None in production
            ..Default::default()
        }
    }

    pub fn enabled_providers(&self) -> Vec<&'static str> {
        let mut v = vec![];
        if self.github.is_some() {
            v.push("github");
        }
        if self.google.is_some() {
            v.push("google");
        }
        v
    }
}

fn env_pair(id: &str, secret: &str) -> Option<ProviderConfig> {
    match (std::env::var(id), std::env::var(secret)) {
        (Ok(c), Ok(s)) if !c.is_empty() && !s.is_empty() => {
            Some(ProviderConfig { client_id: c, client_secret: s })
        }
        _ => None,
    }
}

// ─── Tests ───────────────────────────────────────────────────────────────────

#[cfg(test)]
mod tests {
    use super::*;
    use once_cell::sync::Lazy;
    use std::sync::Mutex;

    static ENV_MUTEX: Lazy<Mutex<()>> = Lazy::new(|| Mutex::new(()));

    #[test]
    fn no_env_means_no_providers() {
        let _lock = ENV_MUTEX.lock().unwrap();
        // Ensure vars are unset
        std::env::remove_var("LITEGEN__OAUTH__GITHUB__CLIENT_ID");
        std::env::remove_var("LITEGEN__OAUTH__GITHUB__CLIENT_SECRET");
        std::env::remove_var("LITEGEN__OAUTH__GOOGLE__CLIENT_ID");
        std::env::remove_var("LITEGEN__OAUTH__GOOGLE__CLIENT_SECRET");

        let c = OAuthConfig::from_env();
        assert!(c.enabled_providers().is_empty(), "should have no providers");
    }

    #[test]
    fn github_env_pair_enables_github() {
        let _lock = ENV_MUTEX.lock().unwrap();
        std::env::set_var("LITEGEN__OAUTH__GITHUB__CLIENT_ID", "gh-client-id");
        std::env::set_var("LITEGEN__OAUTH__GITHUB__CLIENT_SECRET", "gh-client-secret");
        std::env::remove_var("LITEGEN__OAUTH__GOOGLE__CLIENT_ID");
        std::env::remove_var("LITEGEN__OAUTH__GOOGLE__CLIENT_SECRET");

        let c = OAuthConfig::from_env();
        let providers = c.enabled_providers();

        std::env::remove_var("LITEGEN__OAUTH__GITHUB__CLIENT_ID");
        std::env::remove_var("LITEGEN__OAUTH__GITHUB__CLIENT_SECRET");

        assert!(providers.contains(&"github"), "should contain github");
        assert!(!providers.contains(&"google"), "should not contain google");
    }
}
