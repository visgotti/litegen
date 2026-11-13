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
use crate::providers::image::tokenhub::TokenHubImageProvider;
use crate::providers::image::ideogram::IdeogramProvider;
use crate::providers::image::kling::KlingImageProvider;
use crate::providers::image::leonardo::LeonardoImageProvider;
use crate::providers::image::luma::LumaImageProvider;
use crate::providers::image::minimax::MiniMaxImageProvider;
use crate::providers::image::mock::MockProvider;
use crate::providers::image::pixellab::PixelLabProvider;
use crate::providers::image::recraft::RecraftProvider;
use crate::providers::image::runway::RunwayImageProvider;
use crate::providers::image::openai::OpenAiProvider;
use crate::providers::image::replicate::ReplicateProvider;
use crate::providers::image::stability::StabilityProvider;
use crate::providers::image::tripo::TripoImageProvider;
use crate::providers::video::bedrock::BedrockVideoProvider;
use crate::providers::video::bytedance::ByteDanceVideoProvider;
use crate::providers::video::fal::FalVideoProvider;
use crate::providers::video::google::GoogleVideoProvider;
use crate::providers::video::hunyuan::HunyuanVideoProvider;
use crate::providers::video::ideogram::IdeogramVideoProvider;
use crate::providers::video::tokenhub::TokenHubVideoProvider;
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
use crate::providers::model3d::fal::FalModel3dProvider;
use crate::providers::model3d::mock::MockModel3dProvider;
use crate::providers::Model3dProvider;
use crate::providers::{
    ImageProvider, ProviderCredentials, ProviderInstanceConfig, VideoProvider, parse_api_keys,
};
use crate::types::{ApiKeyEntry, ModelInfo, ProviderHealth};

/// Central registry holding all configured image and video providers.
pub struct ProviderRegistry {
    image_providers: RwLock<HashMap<String, Arc<dyn ImageProvider>>>,
    video_providers: RwLock<HashMap<String, Arc<dyn VideoProvider>>>,
    model3d_providers: RwLock<HashMap<String, Arc<dyn Model3dProvider>>>,
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
            model3d_providers: RwLock::new(HashMap::new()),
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
        let model3d = build_model3d_provider(name, &config);

        if image.is_none() && video.is_none() && model3d.is_none() {
            warn!(provider = %name, "Unknown provider, skipping");
            return;
        }

        let has_image = image.is_some();
        let has_video = video.is_some();
        let has_model3d = model3d.is_some();

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
        if let Some(mp) = model3d {
            self.model3d_providers
                .write()
                .await
                .insert(name.to_string(), Arc::from(mp));
        }
        self.provider_configs
            .write()
            .await
            .insert(name.to_string(), config);

        let mut kinds: Vec<&str> = Vec::new();
        if has_image {
            kinds.push("image");
        }
        if has_video {
            kinds.push("video");
        }
        if has_model3d {
            kinds.push("3d");
        }
        info!(provider = %name, modalities = %kinds.join("+"), "Registered provider");
    }

    /// (Re)configure provider `name` at runtime from the management API.
    /// Builds a fresh adapter instance and swaps it into the maps; a job already
    /// in flight keeps the `Arc` it submitted with, so a new key or api_base
    /// applies to new requests only. Returns false for an unknown adapter name.
    pub async fn configure_provider(&self, name: &str, config: ProviderInstanceConfig) -> bool {
        if build_image_provider(name, &config).is_none()
            && build_video_provider(name, &config).is_none()
            && build_model3d_provider(name, &config).is_none()
        {
            return false;
        }
        self.register_provider(name, config).await;
        true
    }

    /// Take provider `name` out of service: new requests see it as
    /// unconfigured; in-flight jobs finish on the instance they hold.
    pub async fn unregister_provider(&self, name: &str) {
        self.image_providers.write().await.remove(name);
        self.video_providers.write().await.remove(name);
        self.model3d_providers.write().await.remove(name);
        self.provider_configs.write().await.remove(name);
    }

    /// The config provider `name` is currently registered with.
    pub async fn registered_config(&self, name: &str) -> Option<ProviderInstanceConfig> {
        self.provider_configs.read().await.get(name).cloned()
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

    /// Build a per-request 3D provider for `name`. See
    /// [`image_provider_for_request`](Self::image_provider_for_request).
    pub async fn model3d_provider_for_request(
        &self,
        name: &str,
        app_creds: Option<ProviderCredentials>,
    ) -> Option<Arc<dyn Model3dProvider>> {
        match app_creds {
            None => self.model3d_provider_for(name).await,
            Some(creds) => {
                let base = self
                    .provider_configs
                    .read()
                    .await
                    .get(name)
                    .cloned()
                    .unwrap_or_default();
                let cfg = base.with_credentials(creds);
                build_model3d_provider(name, &cfg).map(Arc::from)
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

    /// Get a 3D provider by name.
    pub async fn get_model3d_provider(&self, name: &str) -> Option<Arc<dyn Model3dProvider>> {
        self.model3d_providers.read().await.get(name).cloned()
    }

    /// Get an image provider by provider name (same as get_image_provider, for capability-registry routing).
    pub async fn image_provider_for(&self, name: &str) -> Option<Arc<dyn ImageProvider>> {
        self.image_providers.read().await.get(name).cloned()
    }

    /// Get a video provider by provider name (same as get_video_provider, for capability-registry routing).
    pub async fn video_provider_for(&self, name: &str) -> Option<Arc<dyn VideoProvider>> {
        self.video_providers.read().await.get(name).cloned()
    }

    /// Get a 3D provider by provider name (capability-registry routing).
    pub async fn model3d_provider_for(&self, name: &str) -> Option<Arc<dyn Model3dProvider>> {
        self.model3d_providers.read().await.get(name).cloned()
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

    /// The providers configured with the gateway's own keys, by name: each
    /// once, sorted. Reads the maps only; no vendor is called.
    pub async fn configured_provider_names(&self) -> Vec<String> {
        let mut names: Vec<String> = self.image_providers.read().await.keys().cloned().collect();
        names.extend(self.video_providers.read().await.keys().cloned());
        names.extend(self.model3d_providers.read().await.keys().cloned());
        names.sort();
        names.dedup();
        names
    }

    /// Run the health check of every provider configured with the gateway's
    /// own keys: the image entries, then video, then 3D, each naming its
    /// modality.
    ///
    /// The instances are copied out of the maps first, so no read guard is
    /// held while a vendor answers. tokio's `RwLock` queues a new reader
    /// behind a waiting writer, so a guard held across a slow vendor call
    /// stalled a provider change and, behind it, every request's lookup
    /// (P1, code review 2026-11-11). The checks then run side by side: a
    /// sweep takes the slowest vendor's time, not the sum.
    pub async fn health_check_all(&self) -> Vec<ProviderHealth> {
        let image: Vec<(String, Arc<dyn ImageProvider>)> =
            self.image_providers.read().await.iter().map(|(n, p)| (n.clone(), p.clone())).collect();
        let video: Vec<(String, Arc<dyn VideoProvider>)> =
            self.video_providers.read().await.iter().map(|(n, p)| (n.clone(), p.clone())).collect();
        let model3d: Vec<(String, Arc<dyn Model3dProvider>)> =
            self.model3d_providers.read().await.iter().map(|(n, p)| (n.clone(), p.clone())).collect();
        let now = chrono::Utc::now();
        let (image, video, model3d) = tokio::join!(
            futures::future::join_all(image.into_iter().map(|(name, p)| async move { (name, p.health_check().await) })),
            futures::future::join_all(video.into_iter().map(|(name, p)| async move { (name, p.health_check().await) })),
            // 3D instances are separate objects with their own checks (fal's 3D
            // adapter validates its key against fal's Platform API); a vendor that
            // serves only 3D would otherwise never appear in `/health` at all.
            futures::future::join_all(model3d.into_iter().map(|(name, p)| async move { (name, p.health_check().await) })),
        );
        use crate::types::MediaType;
        let entry = |modality: MediaType, (provider, hc): (String, crate::providers::HealthCheckResult)| ProviderHealth {
            provider,
            modality,
            healthy: hc.healthy,
            message: Some(hc.message),
            latency_ms: hc.latency_ms,
            last_checked: Some(now),
        };
        image
            .into_iter()
            .map(|r| entry(MediaType::Image, r))
            .chain(video.into_iter().map(|r| entry(MediaType::Video, r)))
            .chain(model3d.into_iter().map(|r| entry(MediaType::Model3d, r)))
            .collect()
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

    /// Test helper: directly register a pre-built 3D provider.
    #[cfg(test)]
    pub async fn register_mock_model3d(&self, provider: Arc<dyn Model3dProvider>) {
        self.model3d_providers
            .write()
            .await
            .insert("mock".to_string(), provider);
    }
}

/// Pure factory: build the image-provider implementation for `name`, configured
/// with `config`. Returns `None` for vendors without an image provider (or an
/// unknown name). Does NOT touch the registry maps — used both by
/// `register_provider` (global instances) and `image_provider_for_request`
/// (fresh per-request BYO instances).
pub(crate) fn build_image_provider(
    name: &str,
    config: &ProviderInstanceConfig,
) -> Option<Box<dyn ImageProvider>> {
    // OpenAI/Replicate/Google/Fal/Runway/Luma/MiniMax/Hunyuan/TokenHub/Bedrock/
    // Kling/Leonardo/ByteDance/Mock serve images (some also video — see
    // build_video_provider). Stability/Bfl/Ideogram/Recraft are image-only;
    // Tripo's images are its multiview views (its 3D is build_model3d_provider).
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
        "pixellab" => configured!(PixelLabProvider),
        "minimax" => configured!(MiniMaxImageProvider),
        "hunyuan" => configured!(HunyuanImageProvider),
        // Tencent TokenHub: Hunyuan's new home, a Bearer-key API of its own
        // (the old Tencent Cloud API stays as `hunyuan` for its buyers).
        "tokenhub" => configured!(TokenHubImageProvider),
        "bedrock" => configured!(BedrockImageProvider),
        "kling" => configured!(KlingImageProvider),
        "leonardo" => configured!(LeonardoImageProvider),
        "bytedance" => configured!(ByteDanceImageProvider),
        "tripo" => configured!(TripoImageProvider),
        "mock" => configured!(MockProvider),
        _ => None,
    }
}

/// Pure factory: build the video-provider implementation for `name`. Returns
/// `None` for image-only vendors (or an unknown name). Mirror of
/// [`build_image_provider`].
pub(crate) fn build_video_provider(
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
        "tokenhub" => configured!(TokenHubVideoProvider),
        "ideogram" => configured!(IdeogramVideoProvider),
        "bedrock" => configured!(BedrockVideoProvider),
        "kling" => configured!(KlingVideoProvider),
        "leonardo" => configured!(LeonardoVideoProvider),
        "pixverse" => configured!(PixverseProvider),
        "bytedance" => configured!(ByteDanceVideoProvider),
        "mock" => configured!(MockVideoProvider),
        _ => None,
    }
}

/// Pure factory: build the 3D-provider implementation for `name`. Returns
/// `None` for vendors without a 3D implementation (or an unknown name). Mirror
/// of [`build_image_provider`].
pub(crate) fn build_model3d_provider(
    name: &str,
    config: &ProviderInstanceConfig,
) -> Option<Box<dyn Model3dProvider>> {
    macro_rules! configured {
        ($ty:ty) => {{
            let mut p = <$ty>::new();
            p.configure(config.clone());
            Some(Box::new(p) as Box<dyn Model3dProvider>)
        }};
    }
    match name {
        "mock" => configured!(MockModel3dProvider),
        "fal" => configured!(FalModel3dProvider),
        "tripo" => configured!(crate::providers::model3d::tripo::TripoModel3dProvider),
        "meshy" => configured!(crate::providers::model3d::meshy::MeshyModel3dProvider),
        _ => None,
    }
}

/// Canonical list of providers with an image implementation. Keep in sync with
/// [`build_image_provider`]'s match arms — the `provider_catalog_covers_registry`
/// test asserts every name here actually builds.
pub const IMAGE_PROVIDERS: &[&str] = &[
    "openai", "stability", "replicate", "google", "fal", "runway", "luma", "bfl",
    "ideogram", "recraft", "minimax", "hunyuan", "tokenhub", "bedrock", "kling", "leonardo",
    "bytedance", "tripo", "pixellab", "mock",
];

/// Canonical list of providers with a video implementation. Mirrors
/// [`build_video_provider`].
pub const VIDEO_PROVIDERS: &[&str] = &[
    "openai", "replicate", "google", "fal", "runway", "luma", "minimax", "vidu",
    "hunyuan", "tokenhub", "ideogram", "bedrock", "kling", "leonardo", "pixverse", "bytedance", "mock",
];

/// Canonical list of providers with a 3D implementation. Mirrors
/// [`build_model3d_provider`]. Real vendors (Meshy, Tripo3D, Stability, Rodin)
/// land here in phase 2.
pub const MODEL3D_PROVIDERS: &[&str] = &["mock", "fal", "tripo", "meshy"];

/// Build the provider catalog that drives the dashboard credential form. Each
/// entry says which media a provider serves, which credential fields to collect,
/// and which JSON array to submit them in — `api_keys` for bearer schemes,
/// `credential_sets` for signing schemes (Bedrock/Hunyuan/Kling). The dashboard
/// renders dynamic weighted rows generically from this, so it never drifts.
pub fn provider_catalog() -> Vec<crate::types::ProviderCatalogEntry> {
    use crate::types::{CredentialFieldSpec, ProviderCatalogEntry};

    // Union of image+video+model3d names, de-duplicated and stably ordered.
    let mut names: Vec<&str> = Vec::new();
    for n in IMAGE_PROVIDERS
        .iter()
        .chain(VIDEO_PROVIDERS.iter())
        .chain(MODEL3D_PROVIDERS.iter())
    {
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
            if MODEL3D_PROVIDERS.contains(&name) {
                modalities.push("model3d".to_string());
            }
            // Only the two signing providers need a credential set; everything
            // else is a single api_key. Kling joined the api_key providers on
            // 2026-11-03: "API Key authentication must be used to call Kling AI
            // API" (its access/secret key pair works only on the legacy design).
            // @see <https://kling.ai/document-api/api/get-started/authentication.md>
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

/// `model_mapping` keys that would do nothing, one line each: a key naming
/// no catalog model of its provider (by full or bare id — a typo, a retired
/// model), or a provider or row whose vendor takes no model id
/// ([`crate::providers::NO_MODEL_ID`]). `main` logs them at startup.
pub fn model_mapping_warnings(
    providers: &HashMap<String, ProviderEnvConfig>,
    catalog: &crate::capabilities::CapabilityRegistry,
) -> Vec<String> {
    let exempt = |id: &str| crate::providers::NO_MODEL_ID.iter().find(|(k, _)| *k == id).map(|(_, why)| *why);
    let mut names: Vec<&String> = providers.keys().collect();
    names.sort();
    let mut out = Vec::new();
    for name in names {
        let mut keys: Vec<&String> = providers[name].model_mapping.keys().collect();
        keys.sort();
        if keys.is_empty() {
            continue;
        }
        if let Some(why) = exempt(name) {
            out.push(format!("providers.{name}.model_mapping is unused: {name} takes no model id ({why})"));
            continue;
        }
        for key in keys {
            let row = catalog
                .for_provider(name)
                .find(|m| m.id == *key || m.id.split_once('/').is_some_and(|(_, bare)| bare == key.as_str()));
            match row {
                None => out.push(format!("providers.{name}.model_mapping key '{key}' names no {name} model")),
                Some(m) => {
                    if let Some(why) = exempt(&m.id) {
                        out.push(format!("providers.{name}.model_mapping key '{key}' is unused: {} takes no model id ({why})", m.id));
                    }
                }
            }
        }
    }
    out
}

pub(crate) fn build_instance_config(env_config: &ProviderEnvConfig) -> ProviderInstanceConfig {
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

    /// A `model_mapping` key that names no model of its provider (a typo, a
    /// retired id), or one whose vendor takes no model id, is reported at
    /// startup: it would otherwise do nothing, silently.
    #[test]
    fn model_mapping_warnings_name_keys_that_do_nothing() {
        let mut p = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        p.pop();
        p.push("models");
        let catalog = crate::capabilities::CapabilityRegistry::from_dir(&p).unwrap();
        let provider = |mapping: serde_json::Value| -> ProviderEnvConfig {
            serde_json::from_value(serde_json::json!({ "api_key": "k", "model_mapping": mapping })).unwrap()
        };
        let mut providers = HashMap::new();
        providers.insert("bfl".to_string(), provider(serde_json::json!({ "bfl/flux-pro-1.1": "a", "flux-dev": "b", "bfl/flux-typo": "c" })));
        providers.insert("ideogram".to_string(), provider(serde_json::json!({ "ideogram/ideogram-v4": "x" })));
        let warnings = model_mapping_warnings(&providers, &catalog);
        assert_eq!(warnings.len(), 2, "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("bfl/flux-typo") && w.contains("no bfl model")), "{warnings:?}");
        assert!(warnings.iter().any(|w| w.contains("ideogram") && w.contains("takes no model id")), "{warnings:?}");
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

    /// Kling takes an API key since 2026-11-03 ("API Key authentication must be
    /// used to call Kling AI API"; its access/secret key pair works only on the
    /// legacy design litegen left): the dashboard's form is one `key`, pooled
    /// as `api_keys`, and a stored pool configures both Kling adapters.
    /// @see <https://kling.ai/document-api/api/get-started/authentication.md> (2026-11-03)
    #[tokio::test]
    async fn kling_takes_an_api_key_pool() {
        let kling = provider_catalog().into_iter().find(|e| e.name == "kling").unwrap();
        assert_eq!((kling.auth_scheme.as_str(), kling.pool_field.as_str()), ("api_key", "api_keys"));
        assert_eq!(kling.fields.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), ["key"]);
        let creds = || ProviderCredentials::from_json(&serde_json::json!({ "api_keys": [{ "key": "kling-key" }] }));
        let reg = ProviderRegistry::new();
        assert!(reg.image_provider_for_request("kling", Some(creds())).await.expect("image").is_configured());
        assert!(reg.video_provider_for_request("kling", Some(creds())).await.expect("video").is_configured());
    }

    /// TokenHub (Tencent's model hub, a provider of its own beside `hunyuan`,
    /// the old Tencent Cloud API): one Bearer API key — "鉴权方式为请求头
    /// Authorization: Bearer YOUR_API_KEY" — so the dashboard's generic key form,
    /// and a stored pool configures its adapters.
    /// @see <https://cloud.tencent.com/document/product/1823/135745> (2026-11-04)
    #[tokio::test]
    async fn tokenhub_takes_an_api_key_pool() {
        let th = provider_catalog().into_iter().find(|e| e.name == "tokenhub").expect("tokenhub in the catalog");
        assert_eq!((th.auth_scheme.as_str(), th.pool_field.as_str()), ("api_key", "api_keys"));
        assert_eq!(th.fields.iter().map(|f| f.key.as_str()).collect::<Vec<_>>(), ["key"]);
        assert!(th.modalities.contains(&"image".to_string()), "{:?}", th.modalities);
        let creds = || ProviderCredentials::from_json(&serde_json::json!({ "api_keys": [{ "key": "th-key" }] }));
        let reg = ProviderRegistry::new();
        assert!(reg.image_provider_for_request("tokenhub", Some(creds())).await.expect("image").is_configured());
        assert!(reg.video_provider_for_request("tokenhub", Some(creds())).await.expect("video").is_configured());
        assert!(th.modalities.contains(&"video".to_string()), "{:?}", th.modalities);
        let hunyuan = provider_catalog().into_iter().find(|e| e.name == "hunyuan").expect("hunyuan stays");
        assert_eq!(hunyuan.auth_scheme, "tencent_tc3", "the old platform keeps its TC3 pair");
    }

    /// Ideogram's one dashboard key (`api_keys`) configures its video adapter
    /// too (2026-11-10, the resold video on v2): no new name, field or env var.
    #[tokio::test]
    async fn ideogram_video_takes_the_same_api_key_pool() {
        let entry = provider_catalog().into_iter().find(|e| e.name == "ideogram").expect("ideogram in the catalog");
        assert_eq!(entry.modalities, vec!["image".to_string(), "video".to_string()]);
        let creds = || ProviderCredentials::from_json(&serde_json::json!({ "api_keys": [{ "key": "id-key" }] }));
        let reg = ProviderRegistry::new();
        assert!(reg.image_provider_for_request("ideogram", Some(creds())).await.expect("image").is_configured());
        assert!(reg.video_provider_for_request("ideogram", Some(creds())).await.expect("video").is_configured());
    }

    /// The catalog must cover exactly the providers the registry can build, and
    /// describe each with at least one credential field + a valid pool field.
    /// This ties the IMAGE_PROVIDERS/VIDEO_PROVIDERS lists to the build matches.
    #[test]
    fn provider_catalog_covers_registry() {
        let default = ProviderInstanceConfig::default();

        for entry in provider_catalog() {
            let builds = build_image_provider(&entry.name, &default).is_some()
                || build_video_provider(&entry.name, &default).is_some()
                || build_model3d_provider(&entry.name, &default).is_some();
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
        for name in IMAGE_PROVIDERS
            .iter()
            .chain(VIDEO_PROVIDERS.iter())
            .chain(MODEL3D_PROVIDERS.iter())
        {
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

    #[test]
    fn model3d_providers_const_matches_the_build_arms() {
        // Same drift guard IMAGE_PROVIDERS/VIDEO_PROVIDERS get: every name in the
        // const must actually build, and nothing may build that isn't listed.
        let default = ProviderInstanceConfig::default();
        for name in MODEL3D_PROVIDERS {
            assert!(
                build_model3d_provider(name, &default).is_some(),
                "MODEL3D_PROVIDERS lists '{name}' but it does not build"
            );
        }
        assert!(build_model3d_provider("definitely-not-a-vendor", &default).is_none());
    }

    #[test]
    fn catalog_reports_the_model3d_modality() {
        let mock = provider_catalog().into_iter().find(|e| e.name == "mock").unwrap();
        assert!(
            mock.modalities.contains(&"model3d".to_string()),
            "mock serves 3D; the dashboard credential form reads modalities: {:?}",
            mock.modalities
        );
        // A vendor with no 3D implementation must not claim the modality.
        let stability = provider_catalog().into_iter().find(|e| e.name == "stability").unwrap();
        assert!(!stability.modalities.contains(&"model3d".to_string()));
    }

    #[tokio::test]
    async fn a_stored_tripo_key_configures_the_tripo_3d_adapter() {
        // The dashboard stores `{"api_keys":[…]}` with no single api_key.
        let reg = ProviderRegistry::new();
        let creds = ProviderCredentials::from_json(&serde_json::json!({ "api_keys": [{ "key": "tsk_1", "weight": 1 }] }));
        let p = reg.model3d_provider_for_request("tripo", Some(creds.clone())).await.expect("tripo builds");
        assert_eq!(p.name(), "tripo");
        assert!(p.is_configured(), "a pooled key is enough");
        let images = reg.image_provider_for_request("tripo", Some(creds)).await.expect("tripo's multiview images build");
        assert!(images.is_configured(), "the same key serves Tripo's images");
        let tripo = provider_catalog().into_iter().find(|e| e.name == "tripo").expect("tripo is in the credential form");
        assert_eq!(tripo.modalities, vec!["image".to_string(), "model3d".to_string()]);
    }

    #[tokio::test]
    async fn a_stored_meshy_key_configures_the_meshy_3d_adapter() {
        let reg = ProviderRegistry::new();
        let creds = ProviderCredentials::from_json(&serde_json::json!({ "api_keys": [{ "key": "msy_1", "weight": 1 }] }));
        let p = reg.model3d_provider_for_request("meshy", Some(creds)).await.expect("meshy builds");
        assert_eq!(p.name(), "meshy");
        assert!(p.is_configured());
        let meshy = provider_catalog().into_iter().find(|e| e.name == "meshy").expect("meshy is in the credential form");
        assert_eq!(meshy.modalities, vec!["model3d".to_string()]);
    }

    #[tokio::test]
    async fn registers_and_resolves_a_model3d_provider() {
        let reg = ProviderRegistry::new();
        reg.register_provider("mock", ProviderInstanceConfig::default()).await;
        assert!(reg.model3d_provider_for("mock").await.is_some());
        assert!(reg.model3d_provider_for("stability").await.is_none());
    }

    #[tokio::test]
    async fn health_covers_providers_that_only_serve_3d() {
        // `/health` and `/health/ready` both read this list; a 3D-only vendor
        // missing from it could never report a revoked key.
        struct Only3d;
        #[async_trait::async_trait]
        impl Model3dProvider for Only3d {
            fn name(&self) -> &str { "mock" }
            fn configure(&mut self, _c: ProviderInstanceConfig) {}
            fn is_configured(&self) -> bool { true }
            async fn generate(
                &self,
                _m: &crate::capabilities::ModelSchema,
                _b: &crate::types::BaseGenerationRequest,
                _e: &crate::providers::Model3dExtras,
                _r: &crate::proxy::materializer::MaterializedRequest,
            ) -> Result<crate::providers::Model3dGenerationHandle, crate::providers::ProviderError> {
                unreachable!()
            }
            async fn poll_status(
                &self,
                _h: &crate::providers::Model3dGenerationHandle,
            ) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
                unreachable!()
            }
            async fn estimate_cost(
                &self,
                _m: &crate::capabilities::ModelSchema,
                _r: &crate::types::Model3dGenerationRequest,
            ) -> Result<crate::types::CostEstimate, crate::providers::ProviderError> {
                unreachable!()
            }
            async fn health_check(&self) -> crate::providers::HealthCheckResult {
                crate::providers::HealthCheckResult { healthy: false, message: "3d key revoked".into(), latency_ms: None }
            }
        }

        let registry = ProviderRegistry::new();
        registry.register_mock_model3d(Arc::new(Only3d)).await;
        let health = registry.health_check_all().await;
        assert_eq!(health.len(), 1, "{health:?}");
        assert!(!health[0].healthy);
        assert_eq!(health[0].message.as_deref(), Some("3d key revoked"));
    }

    /// How a stand-in vendor's health check waits (P1, code review 2026-11-11).
    enum Hold {
        /// Says it started, then waits to be released: a slow vendor.
        Until { started: Arc<tokio::sync::Notify>, release: Arc<tokio::sync::Notify> },
        /// Waits until every check sharing the barrier has started.
        Together(Arc<tokio::sync::Barrier>),
    }

    struct HeldCheck(Hold);

    #[async_trait::async_trait]
    impl ImageProvider for HeldCheck {
        fn name(&self) -> &str { "held" }
        fn configure(&mut self, _c: ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool { true }
        async fn generate(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _b: &crate::types::BaseGenerationRequest,
            _e: &crate::providers::ImageExtras,
            _r: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<crate::providers::GenerationOutput, crate::providers::ProviderError> {
            unreachable!()
        }
        async fn estimate_cost(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _r: &crate::types::ImageGenerationRequest,
        ) -> Result<crate::types::CostEstimate, crate::providers::ProviderError> {
            unreachable!()
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            match &self.0 {
                Hold::Until { started, release } => {
                    started.notify_one();
                    release.notified().await;
                }
                Hold::Together(barrier) => {
                    barrier.wait().await;
                }
            }
            crate::providers::HealthCheckResult { healthy: true, message: "ok".into(), latency_ms: None }
        }
    }

    /// A vendor slow to answer its health check must not hold the registry:
    /// a provider change made meanwhile, and every request's lookup behind
    /// it, go through at once (tokio's RwLock queues a reader behind a
    /// waiting writer, so a read guard held across the vendor call stalled
    /// both until the vendor answered).
    #[tokio::test]
    async fn a_slow_health_check_holds_up_neither_a_provider_change_nor_a_lookup() {
        let reg = Arc::new(ProviderRegistry::new());
        let (started, release) = (Arc::new(tokio::sync::Notify::new()), Arc::new(tokio::sync::Notify::new()));
        reg.register_mock_image(Arc::new(HeldCheck(Hold::Until { started: started.clone(), release: release.clone() }))).await;
        let sweep = tokio::spawn({
            let reg = reg.clone();
            async move { reg.health_check_all().await }
        });
        started.notified().await;
        // An operator changes a provider while the vendor is slow.
        let change = tokio::spawn({
            let reg = reg.clone();
            async move { reg.configure_provider("mock", ProviderInstanceConfig::default()).await }
        });
        // On this current-thread runtime the change runs only once the test
        // yields. Without the sleep the lookup below would take the read lock
        // before the change queued for the write lock, and pass by accident.
        tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        let lookup = tokio::time::timeout(std::time::Duration::from_secs(2), reg.image_provider_for("mock")).await;
        assert!(lookup.is_ok(), "a request's provider lookup waited for a vendor's health check behind a queued provider change");
        let changed = tokio::time::timeout(std::time::Duration::from_secs(2), change).await;
        assert!(matches!(changed, Ok(Ok(true))), "the provider change waited for a vendor's health check: {changed:?}");
        release.notify_one();
        let health = sweep.await.unwrap();
        assert_eq!(health.len(), 1, "the sweep checks the providers it found when it started: {health:?}");
    }

    /// The checks run side by side, so a sweep takes the slowest vendor's
    /// time, not the sum: each check here waits until the other has started.
    #[tokio::test]
    async fn health_checks_run_side_by_side() {
        let reg = ProviderRegistry::new();
        let together = Arc::new(tokio::sync::Barrier::new(2));
        reg.register_image_provider_named("a", Arc::new(HeldCheck(Hold::Together(together.clone())))).await;
        reg.register_image_provider_named("b", Arc::new(HeldCheck(Hold::Together(together)))).await;
        let health = tokio::time::timeout(std::time::Duration::from_secs(2), reg.health_check_all())
            .await
            .expect("health checks ran one after another: a sweep took the sum of its vendors' latencies");
        assert_eq!(health.len(), 2, "{health:?}");
    }

    /// `/health` lists a provider once per modality it serves, and each entry
    /// says which (registry NEW 3): the mock serves all three.
    #[tokio::test]
    async fn health_entries_name_their_modality() {
        let reg = ProviderRegistry::new();
        reg.register_provider("mock", ProviderInstanceConfig::default()).await;
        let health = serde_json::to_value(reg.health_check_all().await).unwrap();
        let mut seen: Vec<(String, String)> = health
            .as_array()
            .unwrap()
            .iter()
            .map(|h| (h["provider"].as_str().unwrap_or("").to_string(), h["modality"].as_str().unwrap_or("").to_string()))
            .collect();
        seen.sort();
        let want: Vec<(String, String)> = [("mock", "image"), ("mock", "model3d"), ("mock", "video")]
            .iter()
            .map(|(p, m)| (p.to_string(), m.to_string()))
            .collect();
        assert_eq!(seen, want, "each health entry names the modality it checked: {health}");
    }

    /// P8 (code review 2026-11-11): an org's own key builds an adapter for
    /// each call, and each adapter built its own HTTP clients, so every call
    /// paid a new TCP (and TLS) connection to the vendor. Three calls on one
    /// org's key must reuse one connection, as calls on the gateway's key do.
    #[tokio::test]
    async fn calls_on_one_byo_credential_reuse_one_vendor_connection() {
        use std::sync::atomic::Ordering;
        let seen = Arc::new(std::sync::Mutex::new(Vec::<String>::new()));
        let app = axum::Router::new().route(
            "/v1/images/generations",
            axum::routing::post({
                let seen = seen.clone();
                move |headers: axum::http::HeaderMap| {
                    let seen = seen.clone();
                    async move {
                        seen.lock().unwrap().push(headers.get("authorization").and_then(|v| v.to_str().ok()).unwrap_or("").to_string());
                        axum::Json(serde_json::json!({ "created": 1, "data": [{ "b64_json": "aW1n" }] }))
                    }
                }
            }),
        );
        let (base, accepted) = crate::util::counting_vendor(app).await;
        let reg = ProviderRegistry::new();
        let mut cfg = cfg_with_key("PLATFORM");
        cfg.api_base = Some(format!("{base}/v1"));
        reg.register_provider("openai", cfg).await;
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        let schema = crate::capabilities::CapabilityRegistry::from_dir(&models).unwrap().get("openai/gpt-image-2").unwrap().clone();
        let base_req: crate::types::BaseGenerationRequest =
            serde_json::from_value(serde_json::json!({ "prompt": "a cat", "model": "openai/gpt-image-2" })).unwrap();
        let extras = crate::providers::ImageExtras {
            vendor_job: Default::default(),
            size: Some("1024x1024".into()), resolution: None, aspect_ratio: None, quality: None, style: None, steps: None,
            guidance_scale: None, strength: None, response_format: "b64_json".to_string(),
            edit: None, views: Vec::new(), extra: None, output_format: None, background: None,
            expand: None, layout: None, mask_prompt: None, preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        };
        for _ in 0..3 {
            let org_key = ProviderCredentials::from_json(&serde_json::json!({ "api_keys": [{ "key": "org-key" }] }));
            let p = reg.image_provider_for_request("openai", Some(org_key)).await.expect("a BYO instance");
            p.generate(&schema, &base_req, &extras, &crate::proxy::materializer::MaterializedRequest::default()).await.expect("generated");
            // A finished HTTP/1 connection goes back to the pool from a
            // spawned task a moment after its body is read.
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let n = accepted.load(Ordering::SeqCst);
        assert_eq!(n, 1, "three calls on one org's key opened {n} connections: each built its own HTTP client");
        assert_eq!(*seen.lock().unwrap(), vec!["Bearer org-key"; 3]);
    }

    /// Readiness reads the providers configured with the gateway's own keys
    /// from the registry, each once and sorted, with no vendor call.
    #[tokio::test]
    async fn configured_provider_names_lists_each_provider_once() {
        let reg = ProviderRegistry::new();
        reg.register_provider("openai", cfg_with_key("K")).await;
        reg.register_provider("mock", ProviderInstanceConfig::default()).await;
        reg.register_provider("meshy", cfg_with_key("K")).await;
        assert_eq!(reg.configured_provider_names().await, ["meshy", "mock", "openai"]);
    }
}
