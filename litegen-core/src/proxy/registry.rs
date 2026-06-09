use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::RwLock;
use tracing::{info, warn};

use crate::config::{AppConfig, ProviderEnvConfig};
use crate::providers::image::bedrock::BedrockImageProvider;
use crate::providers::image::bfl::BflProvider;
use crate::providers::image::bytedance::ByteDanceImageProvider;
use crate::providers::image::fal::FalProvider;
use crate::providers::image::google::GoogleProvider;
use crate::providers::image::hunyuan::HunyuanImageProvider;
use crate::providers::image::ideogram::IdeogramProvider;
use crate::providers::image::kling::KlingImageProvider;
use crate::providers::image::leonardo::LeonardoImageProvider;
use crate::providers::image::luma::LumaImageProvider;
use crate::providers::image::minimax::MiniMaxImageProvider;
use crate::providers::image::mock::MockProvider;
use crate::providers::image::recraft::RecraftProvider;
use crate::providers::image::runway::RunwayImageProvider;
use crate::providers::image::openai::OpenAiProvider;
use crate::providers::image::replicate::ReplicateProvider;
use crate::providers::image::stability::StabilityProvider;
use crate::providers::video::bedrock::BedrockVideoProvider;
use crate::providers::video::bytedance::ByteDanceVideoProvider;
use crate::providers::video::fal::FalVideoProvider;
use crate::providers::video::google::GoogleVideoProvider;
use crate::providers::video::hunyuan::HunyuanVideoProvider;
use crate::providers::video::kling::KlingVideoProvider;
use crate::providers::video::leonardo::LeonardoVideoProvider;
use crate::providers::video::luma::LumaProvider;
use crate::providers::video::minimax::MiniMaxVideoProvider;
use crate::providers::video::pixverse::PixverseProvider;
use crate::providers::video::vidu::ViduProvider;
use crate::providers::video::mock::MockVideoProvider;
use crate::providers::video::openai::OpenAiVideoProvider;
use crate::providers::video::replicate::ReplicateVideoProvider;
use crate::providers::video::runway::RunwayProvider;
use crate::providers::{
    ImageProvider, ProviderCredentials, ProviderInstanceConfig, VideoProvider, parse_api_keys,
};
use crate::types::{ApiKeyEntry, ModelInfo, ProviderHealth};

/// Central registry holding all configured image and video providers.
pub struct ProviderRegistry {
    image_providers: RwLock<HashMap<String, Arc<dyn ImageProvider>>>,
    video_providers: RwLock<HashMap<String, Arc<dyn VideoProvider>>>,
    /// The `ProviderInstanceConfig` each provider was registered with, keyed by
    /// provider name. Used to build per-request override instances that keep the
    /// non-credential fields (api_base, model_mapping, …) while swapping in a
    /// per-app BYO credential.
    provider_configs: RwLock<HashMap<String, ProviderInstanceConfig>>,
}

impl Default for ProviderRegistry {
    fn default() -> Self {
        Self::new()
    }
}

impl ProviderRegistry {
    pub fn new() -> Self {
        Self {
            image_providers: RwLock::new(HashMap::new()),
            video_providers: RwLock::new(HashMap::new()),
            provider_configs: RwLock::new(HashMap::new()),
        }
    }

    /// Initialize all providers from configuration.
    pub async fn init_from_config(&self, config: &AppConfig) {
        for (name, provider_config) in &config.providers {
            if !provider_config.enabled {
                info!(provider = %name, "Provider disabled, skipping");
                continue;
            }
            let instance_config = build_instance_config(provider_config);
            // The mock provider has no real upstream and so needs no credentials —
            // it's useful for local dev, smoke tests, and the e2e tests in this
            // repo. All other providers require credentials. The check is
            // auth-scheme-aware: signing providers (Bedrock/Kling/Hunyuan) supply
            // key_id/key_secret rather than a bare api_key.
            if name != "mock" && !instance_config.credentials.any_present() {
                info!(provider = %name, "No credentials configured, skipping");
                continue;
            }
            self.register_provider(name, instance_config).await;
        }
    }

    /// Register a single provider by name, creating the appropriate implementation.
    ///
    /// Delegates per-name construction to the pure [`build_image_provider`] /
    /// [`build_video_provider`] factories, then inserts the resulting `Arc`s into
    /// the relevant maps. A vendor that serves both modalities (e.g. "openai",
    /// "fal") is inserted into BOTH maps. The config is also stashed in
    /// `provider_configs` so per-request BYO overrides can reuse its
    /// non-credential fields.
    async fn register_provider(&self, name: &str, config: ProviderInstanceConfig) {
        let image = build_image_provider(name, &config);
        let video = build_video_provider(name, &config);

        if image.is_none() && video.is_none() {
            warn!(provider = %name, "Unknown provider, skipping");
            return;
        }

        let has_image = image.is_some();
        let has_video = video.is_some();

        if let Some(ip) = image {
            self.image_providers
                .write()
                .await
                .insert(name.to_string(), Arc::from(ip));
        }
        if let Some(vp) = video {
            self.video_providers
                .write()
                .await
                .insert(name.to_string(), Arc::from(vp));
        }
        self.provider_configs
            .write()
            .await
            .insert(name.to_string(), config);

        match (has_image, has_video) {
            (true, true) => info!(provider = %name, "Registered image+video provider"),
            (true, false) => info!(provider = %name, "Registered image provider"),
            (false, true) => info!(provider = %name, "Registered video provider"),
            (false, false) => {}
        }
    }

    /// Build a per-request image provider for `name`, optionally overriding its
    /// credentials with a per-app BYO credential.
    ///
    /// - `None` → returns the cached global instance (identical to
    ///   [`image_provider_for`](Self::image_provider_for)); behavior unchanged.
    /// - `Some(creds)` → builds a FRESH instance configured with `creds` layered
    ///   over the registered global config (or a default config when no global is
    ///   registered for this provider, so BYO works without a platform key).
    pub async fn image_provider_for_request(
        &self,
        name: &str,
        app_creds: Option<ProviderCredentials>,
    ) -> Option<Arc<dyn ImageProvider>> {
        match app_creds {
            None => self.image_provider_for(name).await,
            Some(creds) => {
                let base = self
                    .provider_configs
                    .read()
                    .await
                    .get(name)
                    .cloned()
                    .unwrap_or_default();
                let cfg = base.with_credentials(creds);
                build_image_provider(name, &cfg).map(Arc::from)
            }
        }
    }

    /// Build a per-request video provider for `name`. See
    /// [`image_provider_for_request`](Self::image_provider_for_request).
    pub async fn video_provider_for_request(
        &self,
        name: &str,
        app_creds: Option<ProviderCredentials>,
    ) -> Option<Arc<dyn VideoProvider>> {
        match app_creds {
            None => self.video_provider_for(name).await,
            Some(creds) => {
                let base = self
                    .provider_configs
                    .read()
                    .await
                    .get(name)
                    .cloned()
                    .unwrap_or_default();
                let cfg = base.with_credentials(creds);
                build_video_provider(name, &cfg).map(Arc::from)
            }
        }
    }

    /// Get an image provider by name.
    pub async fn get_image_provider(&self, name: &str) -> Option<Arc<dyn ImageProvider>> {
        self.image_providers.read().await.get(name).cloned()
    }

    /// Get a video provider by name.
    pub async fn get_video_provider(&self, name: &str) -> Option<Arc<dyn VideoProvider>> {
        self.video_providers.read().await.get(name).cloned()
    }

    /// Get an image provider by provider name (same as get_image_provider, for capability-registry routing).
    pub async fn image_provider_for(&self, name: &str) -> Option<Arc<dyn ImageProvider>> {
        self.image_providers.read().await.get(name).cloned()
    }

    /// Get a video provider by provider name (same as get_video_provider, for capability-registry routing).
    pub async fn video_provider_for(&self, name: &str) -> Option<Arc<dyn VideoProvider>> {
        self.video_providers.read().await.get(name).cloned()
    }

    /// Find an image provider that supports the given model.
    /// Uses provider prefix from model ID (e.g. "openai/dall-e-3" → "openai").
    pub async fn find_image_provider_for_model(
        &self,
        model: &str,
    ) -> Option<(String, Arc<dyn ImageProvider>)> {
        if let Some(slash_pos) = model.find('/') {
            let provider_name = &model[..slash_pos];
            if let Some(p) = self.image_providers.read().await.get(provider_name) {
                return Some((provider_name.to_string(), p.clone()));
            }
        }
        None
    }

    /// Find a video provider that supports the given model.
    pub async fn find_video_provider_for_model(
        &self,
        model: &str,
    ) -> Option<(String, Arc<dyn VideoProvider>)> {
        if let Some(slash_pos) = model.find('/') {
            let provider_name = &model[..slash_pos];
            if let Some(p) = self.video_providers.read().await.get(provider_name) {
                return Some((provider_name.to_string(), p.clone()));
            }
        }
        None
    }

    /// List all available models — placeholder until registry is wired (Phase G).
    pub async fn list_all_models(&self) -> Vec<ModelInfo> {
        Vec::new()
    }

    /// Run health checks on all configured providers.
    pub async fn health_check_all(&self) -> Vec<ProviderHealth> {
        let mut results = Vec::new();
        let now = chrono::Utc::now();

        for (name, provider) in self.image_providers.read().await.iter() {
            let hc = provider.health_check().await;
            results.push(ProviderHealth {
                provider: name.clone(),
                healthy: hc.healthy,
                message: Some(hc.message),
                latency_ms: hc.latency_ms,
                last_checked: Some(now),
            });
        }
        for (name, provider) in self.video_providers.read().await.iter() {
            let hc = provider.health_check().await;
            results.push(ProviderHealth {
                provider: name.clone(),
                healthy: hc.healthy,
                message: Some(hc.message),
                latency_ms: hc.latency_ms,
                last_checked: Some(now),
            });
        }
        results
    }

    pub async fn image_provider_names(&self) -> Vec<String> {
        self.image_providers.read().await.keys().cloned().collect()
    }

    pub async fn video_provider_names(&self) -> Vec<String> {
        self.video_providers.read().await.keys().cloned().collect()
    }

    /// Test helper: directly register a pre-built image provider.
    #[cfg(test)]
    pub async fn register_mock_image(&self, provider: Arc<dyn ImageProvider>) {
        self.image_providers
            .write()
            .await
            .insert("mock".to_string(), provider);
    }

    /// Test helper: directly register a pre-built video provider.
    #[cfg(test)]
    pub async fn register_mock_video(&self, provider: Arc<dyn VideoProvider>) {
        self.video_providers
            .write()
            .await
            .insert("mock".to_string(), provider);
    }

    /// Test helper: register a pre-built image provider under an arbitrary name.
    #[cfg(test)]
    pub async fn register_image_provider_named(&self, name: &str, provider: Arc<dyn ImageProvider>) {
        self.image_providers
            .write()
            .await
            .insert(name.to_string(), provider);
    }
}

/// Pure factory: build the image-provider implementation for `name`, configured
/// with `config`. Returns `None` for vendors without an image provider (or an
/// unknown name). Does NOT touch the registry maps — used both by
/// `register_provider` (global instances) and `image_provider_for_request`
/// (fresh per-request BYO instances).
fn build_image_provider(
    name: &str,
    config: &ProviderInstanceConfig,
) -> Option<Box<dyn ImageProvider>> {
    // OpenAI/Replicate/Google/Fal/Runway/Luma/MiniMax/Hunyuan/Bedrock/Kling/
    // Leonardo/ByteDance/Mock serve images (some also video — see
    // build_video_provider). Stability/Bfl/Ideogram/Recraft are image-only.
    macro_rules! configured {
        ($ty:ty) => {{
            let mut p = <$ty>::new();
            p.configure(config.clone());
            Some(Box::new(p) as Box<dyn ImageProvider>)
        }};
    }
    match name {
        "openai" => configured!(OpenAiProvider),
        "stability" => configured!(StabilityProvider),
        "replicate" => configured!(ReplicateProvider),
        "google" => configured!(GoogleProvider),
        "fal" => configured!(FalProvider),
        "runway" => configured!(RunwayImageProvider),
        "luma" => configured!(LumaImageProvider),
        "bfl" => configured!(BflProvider),
        "ideogram" => configured!(IdeogramProvider),
        "recraft" => configured!(RecraftProvider),
        "minimax" => configured!(MiniMaxImageProvider),
        "hunyuan" => configured!(HunyuanImageProvider),
        "bedrock" => configured!(BedrockImageProvider),
        "kling" => configured!(KlingImageProvider),
        "leonardo" => configured!(LeonardoImageProvider),
        "bytedance" => configured!(ByteDanceImageProvider),
        "mock" => configured!(MockProvider),
        _ => None,
    }
}

/// Pure factory: build the video-provider implementation for `name`. Returns
/// `None` for image-only vendors (or an unknown name). Mirror of
/// [`build_image_provider`].
fn build_video_provider(
    name: &str,
    config: &ProviderInstanceConfig,
) -> Option<Box<dyn VideoProvider>> {
    macro_rules! configured {
        ($ty:ty) => {{
            let mut p = <$ty>::new();
            p.configure(config.clone());
            Some(Box::new(p) as Box<dyn VideoProvider>)
        }};
    }
    match name {
        "openai" => configured!(OpenAiVideoProvider),
        "replicate" => configured!(ReplicateVideoProvider),
        "google" => configured!(GoogleVideoProvider),
        "fal" => configured!(FalVideoProvider),
        "runway" => configured!(RunwayProvider),
        "luma" => configured!(LumaProvider),
        "minimax" => configured!(MiniMaxVideoProvider),
        "vidu" => configured!(ViduProvider),
        "hunyuan" => configured!(HunyuanVideoProvider),
        "bedrock" => configured!(BedrockVideoProvider),
        "kling" => configured!(KlingVideoProvider),
        "leonardo" => configured!(LeonardoVideoProvider),
        "pixverse" => configured!(PixverseProvider),
        "bytedance" => configured!(ByteDanceVideoProvider),
        "mock" => configured!(MockVideoProvider),
        _ => None,
    }
}

/// Canonical list of providers with an image implementation. Keep in sync with
/// [`build_image_provider`]'s match arms — the `provider_catalog_covers_registry`
/// test asserts every name here actually builds.
pub const IMAGE_PROVIDERS: &[&str] = &[
    "openai", "stability", "replicate", "google", "fal", "runway", "luma", "bfl",
    "ideogram", "recraft", "minimax", "hunyuan", "bedrock", "kling", "leonardo",
    "bytedance", "mock",
];

/// Canonical list of providers with a video implementation. Mirrors
/// [`build_video_provider`].
pub const VIDEO_PROVIDERS: &[&str] = &[
    "openai", "replicate", "google", "fal", "runway", "luma", "minimax", "vidu",
    "hunyuan", "bedrock", "kling", "leonardo", "pixverse", "bytedance", "mock",
];

/// Build the provider catalog that drives the dashboard credential form. Each
/// entry says which media a provider serves, which credential fields to collect,
/// and which JSON array to submit them in — `api_keys` for bearer schemes,
/// `credential_sets` for signing schemes (Bedrock/Hunyuan/Kling). The dashboard
/// renders dynamic weighted rows generically from this, so it never drifts.
pub fn provider_catalog() -> Vec<crate::types::ProviderCatalogEntry> {
    use crate::types::{CredentialFieldSpec, ProviderCatalogEntry};

    // Union of image+video names, de-duplicated and stably ordered.
    let mut names: Vec<&str> = Vec::new();
    for n in IMAGE_PROVIDERS.iter().chain(VIDEO_PROVIDERS.iter()) {
        if !names.contains(n) {
            names.push(n);
        }
    }
    names.sort_unstable();

    let field = |key: &str, label: &str, secret: bool, optional: bool| CredentialFieldSpec {
        key: key.to_string(),
        label: label.to_string(),
        secret,
        optional,
    };

    names
        .into_iter()
        .map(|name| {
            let mut modalities = Vec::new();
            if IMAGE_PROVIDERS.contains(&name) {
                modalities.push("image".to_string());
            }
            if VIDEO_PROVIDERS.contains(&name) {
                modalities.push("video".to_string());
            }
            // Only the three signing providers need a credential set; everything
            // else is a single bearer api_key.
            let (auth_scheme, pool_field, fields) = match name {
                "bedrock" => (
                    "aws_sigv4",
                    "credential_sets",
                    vec![
                        field("key_id", "Access key ID", false, false),
                        field("key_secret", "Secret access key", true, false),
                        field("region", "Region", false, true),
                    ],
                ),
                "hunyuan" => (
                    "tencent_tc3",
                    "credential_sets",
                    vec![
                        field("key_id", "Secret ID", false, false),
                        field("key_secret", "Secret key", true, false),
                        field("region", "Region", false, true),
                    ],
                ),
                "kling" => (
                    "kling_jwt",
                    "credential_sets",
                    vec![
                        field("key_id", "Access key", false, false),
                        field("key_secret", "Secret key", true, false),
                    ],
                ),
                _ => ("api_key", "api_keys", vec![field("key", "API key", true, false)]),
            };
            ProviderCatalogEntry {
                name: name.to_string(),
                modalities,
                auth_scheme: auth_scheme.to_string(),
                pool_field: pool_field.to_string(),
                fields,
            }
        })
        .collect()
}

fn build_instance_config(env_config: &ProviderEnvConfig) -> ProviderInstanceConfig {
    let api_key = env_config.api_key.clone().unwrap_or_default();
    let api_keys = env_config
        .api_keys
        .as_deref()
        .map(parse_api_keys)
        .unwrap_or_else(|| {
            if api_key.is_empty() {
                Vec::new()
            } else {
                vec![ApiKeyEntry {
                    key: api_key.clone(),
                    weight: 1,
                    label: None,
                }]
            }
        });

    let credentials = crate::providers::ProviderCredentials {
        api_key: if api_key.is_empty() { None } else { Some(api_key.clone()) },
        api_keys: api_keys.clone(),
        // Signing credential pools arrive via per-app BYO credentials, not env.
        credential_sets: Vec::new(),
        key_id: env_config.key_id.clone().filter(|s| !s.is_empty()),
        key_secret: env_config.key_secret.clone().filter(|s| !s.is_empty()),
        region: env_config.region.clone().filter(|s| !s.is_empty()),
        extra: env_config.credentials_extra.clone(),
    };

    ProviderInstanceConfig {
        api_key,
        api_keys,
        api_base: env_config.api_base.clone(),
        model_mapping: env_config.model_mapping.clone(),
        extra_headers: env_config.extra_headers.clone(),
        options: env_config.options.clone(),
        credentials,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Build a minimal global config carrying a single api_key.
    fn cfg_with_key(key: &str) -> ProviderInstanceConfig {
        let creds = ProviderCredentials {
            api_key: Some(key.to_string()),
            ..Default::default()
        };
        ProviderInstanceConfig {
            api_key: key.to_string(),
            credentials: creds,
            ..Default::default()
        }
    }

    /// `with_credentials` swaps the derived api_key + credentials, keeping the
    /// non-credential fields. This is the per-request override the registry uses.
    #[test]
    fn with_credentials_swaps_api_key() {
        let mut base = cfg_with_key("GLOBAL");
        base.api_base = Some("https://custom.example/v1".to_string());

        let app = ProviderCredentials {
            api_key: Some("APPKEY".to_string()),
            ..Default::default()
        };
        let overridden = base.with_credentials(app);

        assert_eq!(overridden.api_key, "APPKEY");
        assert_eq!(overridden.credentials.api_key.as_deref(), Some("APPKEY"));
        // Non-credential field preserved.
        assert_eq!(overridden.api_base.as_deref(), Some("https://custom.example/v1"));
    }

    /// With a registered global "openai", `image_provider_for_request(.., None)`
    /// returns the SAME cached Arc as `image_provider_for` (behavior unchanged),
    /// while `Some(app_creds)` returns a DISTINCT, freshly-built instance.
    #[tokio::test]
    async fn image_provider_for_request_override_uses_app_key() {
        let reg = ProviderRegistry::new();
        reg.register_provider("openai", cfg_with_key("GLOBAL")).await;

        // None → cached global instance (identity-equal to image_provider_for).
        let global = reg.image_provider_for("openai").await.expect("global registered");
        let via_none = reg
            .image_provider_for_request("openai", None)
            .await
            .expect("none returns global");
        assert!(
            Arc::ptr_eq(&global, &via_none),
            "None must return the cached global Arc (unchanged behavior)"
        );

        // Some(app_creds) → fresh per-request instance (different allocation),
        // still configured.
        let app = ProviderCredentials {
            api_key: Some("APPKEY".to_string()),
            ..Default::default()
        };
        let via_override = reg
            .image_provider_for_request("openai", Some(app))
            .await
            .expect("override builds an instance");
        assert!(
            !Arc::ptr_eq(&global, &via_override),
            "override must be a fresh instance, not the cached global"
        );
        assert!(via_override.is_configured(), "override instance is configured");
        assert_eq!(via_override.name(), "openai");
    }

    /// BYO works even when no global "openai" is registered: the override path
    /// builds a fresh instance from default config + the app key, and it reports
    /// configured.
    #[tokio::test]
    async fn image_provider_for_request_byo_without_global() {
        let reg = ProviderRegistry::new();
        // No register_provider call — there is no global "openai".
        assert!(reg.image_provider_for("openai").await.is_none());

        let app = ProviderCredentials {
            api_key: Some("K".to_string()),
            ..Default::default()
        };
        let prov = reg
            .image_provider_for_request("openai", Some(app))
            .await
            .expect("BYO builds an instance without a global");
        assert!(prov.is_configured(), "BYO instance is configured with just the app key");
    }

    /// Video override path mirrors the image path: None → cached global,
    /// Some → fresh instance; and BYO works without a global.
    #[tokio::test]
    async fn video_provider_for_request_override_and_byo() {
        let reg = ProviderRegistry::new();
        reg.register_provider("openai", cfg_with_key("GLOBAL")).await;

        let global = reg.video_provider_for("openai").await.expect("global video registered");
        let via_none = reg
            .video_provider_for_request("openai", None)
            .await
            .expect("none returns global video");
        assert!(Arc::ptr_eq(&global, &via_none));

        let app = ProviderCredentials {
            api_key: Some("APPKEY".to_string()),
            ..Default::default()
        };
        let via_override = reg
            .video_provider_for_request("openai", Some(app.clone()))
            .await
            .expect("override builds a video instance");
        assert!(!Arc::ptr_eq(&global, &via_override));
        assert!(via_override.is_configured());

        // BYO without a global, on a fresh registry.
        let reg2 = ProviderRegistry::new();
        let byo = reg2
            .video_provider_for_request("openai", Some(app))
            .await
            .expect("BYO video instance without a global");
        assert!(byo.is_configured());
    }

    /// The factory refactor must register a provider into BOTH the image and
    /// video maps when the vendor serves both modalities.
    #[tokio::test]
    async fn register_provider_populates_both_maps_for_dual_vendor() {
        let reg = ProviderRegistry::new();
        reg.register_provider("openai", cfg_with_key("GLOBAL")).await;
        assert!(reg.image_provider_for("openai").await.is_some());
        assert!(reg.video_provider_for("openai").await.is_some());

        // Image-only vendor: present in image map, absent from video map.
        reg.register_provider("stability", cfg_with_key("GLOBAL")).await;
        assert!(reg.image_provider_for("stability").await.is_some());
        assert!(reg.video_provider_for("stability").await.is_none());

        // Video-only vendor: present in video map, absent from image map.
        reg.register_provider("pixverse", cfg_with_key("GLOBAL")).await;
        assert!(reg.video_provider_for("pixverse").await.is_some());
        assert!(reg.image_provider_for("pixverse").await.is_none());
    }

    /// BYO with a weighted multi-key pool and NO single api_key: the per-request
    /// instance is configured purely from the pool, proving with_credentials →
    /// configure → ApiKeyPool flows end to end.
    #[tokio::test]
    async fn image_provider_for_request_byo_weighted_pool() {
        let reg = ProviderRegistry::new();
        let app = ProviderCredentials {
            api_key: None,
            api_keys: vec![
                crate::types::ApiKeyEntry { key: "sk-1".into(), weight: 3, label: None },
                crate::types::ApiKeyEntry { key: "sk-2".into(), weight: 1, label: None },
            ],
            ..Default::default()
        };
        let prov = reg
            .image_provider_for_request("openai", Some(app))
            .await
            .expect("BYO builds an instance from a key pool alone");
        assert!(prov.is_configured(), "a multi-key pool alone configures the provider");
    }

    /// The full stored-credential chain: decrypted JSON (as persisted in
    /// `provider_credentials`) → `from_json` → registry → a configured, pooled
    /// provider. This is the path a multi-tenant BYO customer exercises.
    #[tokio::test]
    async fn byo_from_stored_json_builds_pooled_provider() {
        let creds = ProviderCredentials::from_json(&serde_json::json!({
            "api_keys": [{"key": "sk-1", "weight": 2}, {"key": "sk-2"}]
        }));
        assert_eq!(creds.api_keys.len(), 2, "stored JSON yields a two-key pool");

        let reg = ProviderRegistry::new();
        let prov = reg
            .image_provider_for_request("openai", Some(creds))
            .await
            .expect("stored-JSON multi-key creds build a configured provider");
        assert!(prov.is_configured());
    }

    /// The signing-scheme analogue: a weighted `credential_sets` pool from stored
    /// JSON → `from_json` → registry → a configured Bedrock (SigV4) provider,
    /// with no single key_id/key_secret present.
    #[tokio::test]
    async fn byo_signing_credential_pool_builds_configured_provider() {
        let creds = ProviderCredentials::from_json(&serde_json::json!({
            "credential_sets": [
                {"key_id": "AKIA1", "key_secret": "s1", "region": "us-east-1", "weight": 2},
                {"key_id": "AKIA2", "key_secret": "s2", "region": "eu-west-1"}
            ]
        }));
        assert_eq!(creds.credential_sets.len(), 2);
        assert!(creds.key_id.is_none(), "pool-only: no single credential");

        let reg = ProviderRegistry::new();
        let prov = reg
            .image_provider_for_request("bedrock", Some(creds))
            .await
            .expect("stored-JSON credential_sets build a configured signing provider");
        assert!(prov.is_configured(), "a credential pool alone configures the signing provider");
    }

    /// The catalog must cover exactly the providers the registry can build, and
    /// describe each with at least one credential field + a valid pool field.
    /// This ties the IMAGE_PROVIDERS/VIDEO_PROVIDERS lists to the build matches.
    #[test]
    fn provider_catalog_covers_registry() {
        let default = ProviderInstanceConfig::default();

        for entry in provider_catalog() {
            let builds = build_image_provider(&entry.name, &default).is_some()
                || build_video_provider(&entry.name, &default).is_some();
            assert!(builds, "catalog provider '{}' does not build in the registry", entry.name);
            assert!(!entry.fields.is_empty(), "{} exposes no credential fields", entry.name);
            assert!(
                entry.pool_field == "api_keys" || entry.pool_field == "credential_sets",
                "{} has an unexpected pool_field {}",
                entry.name,
                entry.pool_field
            );
        }

        // Every registered provider name appears in the catalog (no drift).
        let catalog: std::collections::BTreeSet<String> =
            provider_catalog().into_iter().map(|e| e.name).collect();
        for name in IMAGE_PROVIDERS.iter().chain(VIDEO_PROVIDERS.iter()) {
            assert!(catalog.contains(*name), "registered provider '{name}' missing from catalog");
        }

        // Signing providers expose a credential set (key_id/key_secret), not an
        // api_key; the region field is optional. Bearer providers expose `key`.
        let bedrock = provider_catalog().into_iter().find(|e| e.name == "bedrock").unwrap();
        assert_eq!(bedrock.pool_field, "credential_sets");
        assert_eq!(bedrock.modalities, vec!["image".to_string(), "video".to_string()]);
        assert!(bedrock.fields.iter().any(|f| f.key == "key_id"));
        assert!(bedrock.fields.iter().any(|f| f.key == "region" && f.optional));

        let openai = provider_catalog().into_iter().find(|e| e.name == "openai").unwrap();
        assert_eq!(openai.pool_field, "api_keys");
        assert_eq!(openai.fields.len(), 1);
        assert_eq!(openai.fields[0].key, "key");
        assert!(openai.fields[0].secret);

        // Image-only and video-only vendors report a single modality.
        let stability = provider_catalog().into_iter().find(|e| e.name == "stability").unwrap();
        assert_eq!(stability.modalities, vec!["image".to_string()]);
        let pixverse = provider_catalog().into_iter().find(|e| e.name == "pixverse").unwrap();
        assert_eq!(pixverse.modalities, vec!["video".to_string()]);
    }
}
