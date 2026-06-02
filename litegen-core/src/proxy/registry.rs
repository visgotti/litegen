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
    ImageProvider, ProviderInstanceConfig, VideoProvider, parse_api_keys,
};
use crate::types::{ApiKeyEntry, ModelInfo, ProviderHealth};

/// Central registry holding all configured image and video providers.
pub struct ProviderRegistry {
    image_providers: RwLock<HashMap<String, Arc<dyn ImageProvider>>>,
    video_providers: RwLock<HashMap<String, Arc<dyn VideoProvider>>>,
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
    async fn register_provider(&self, name: &str, config: ProviderInstanceConfig) {
        match name {
            "openai" => {
                // OpenAI serves BOTH images (DALL-E) and video (Sora) under the
                // single vendor name "openai", so models like `openai/dall-e-3`
                // and `openai/sora` both resolve to this provider.
                let mut ip = OpenAiProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = OpenAiVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "openai", "Registered image+video provider");
            }
            "stability" => {
                let mut p = StabilityProvider::new();
                p.configure(config);
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(p));
                info!(provider = "stability", "Registered image provider");
            }
            "replicate" => {
                // Replicate serves BOTH images (Flux/SDXL/SD3) and video
                // (AnimateDiff/SVD/Zeroscope) under the single vendor name.
                let mut ip = ReplicateProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = ReplicateVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "replicate", "Registered image+video provider");
            }
            "google" => {
                // Google serves images (Imagen/Gemini) and video (Veo) under the
                // single vendor name "google".
                let mut ip = GoogleProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = GoogleVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "google", "Registered image+video provider");
            }
            "fal" => {
                // Fal serves BOTH images (Flux/SDXL/etc.) and video (Kling/
                // MiniMax/SVD/LTX) under the single vendor name "fal".
                let mut ip = FalProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = FalVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "fal", "Registered image+video provider");
            }
            "runway" => {
                // Runway serves video (Gen-3/4) and image (Gen-4 image).
                let mut vp = RunwayProvider::new();
                vp.configure(config.clone());
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));

                let mut ip = RunwayImageProvider::new();
                ip.configure(config);
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));
                info!(provider = "runway", "Registered image+video provider");
            }
            "luma" => {
                // Luma serves video (Dream Machine/Ray) and image (Photon).
                let mut vp = LumaProvider::new();
                vp.configure(config.clone());
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));

                let mut ip = LumaImageProvider::new();
                ip.configure(config);
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));
                info!(provider = "luma", "Registered image+video provider");
            }
            "bfl" => {
                let mut p = BflProvider::new();
                p.configure(config);
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(p));
                info!(provider = "bfl", "Registered image provider");
            }
            "ideogram" => {
                let mut p = IdeogramProvider::new();
                p.configure(config);
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(p));
                info!(provider = "ideogram", "Registered image provider");
            }
            "recraft" => {
                let mut p = RecraftProvider::new();
                p.configure(config);
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(p));
                info!(provider = "recraft", "Registered image provider");
            }
            "minimax" => {
                // MiniMax serves image (image-01) and video (Hailuo) under one name.
                let mut ip = MiniMaxImageProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = MiniMaxVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "minimax", "Registered image+video provider");
            }
            "vidu" => {
                let mut p = ViduProvider::new();
                p.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(p));
                info!(provider = "vidu", "Registered video provider");
            }
            "hunyuan" => {
                // Tencent Hunyuan: image + vclm video; TC3-HMAC-SHA256 signing.
                let mut ip = HunyuanImageProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = HunyuanVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "hunyuan", "Registered image+video provider");
            }
            "bedrock" => {
                // Amazon Bedrock Nova: Canvas (image) + Reel (video); AWS SigV4.
                let mut ip = BedrockImageProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = BedrockVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "bedrock", "Registered image+video provider");
            }
            "kling" => {
                // Kling serves image (Kolors) and video under one name; JWT auth.
                let mut ip = KlingImageProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = KlingVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "kling", "Registered image+video provider");
            }
            "leonardo" => {
                // Leonardo serves image (generations) and video (image-to-video).
                let mut ip = LeonardoImageProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = LeonardoVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "leonardo", "Registered image+video provider");
            }
            "pixverse" => {
                let mut p = PixverseProvider::new();
                p.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(p));
                info!(provider = "pixverse", "Registered video provider");
            }
            "bytedance" => {
                // ByteDance serves image (Seedream) and video (Seedance) under one name.
                let mut ip = ByteDanceImageProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = ByteDanceVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "bytedance", "Registered image+video provider");
            }
            "mock" => {
                let mut ip = MockProvider::new();
                ip.configure(config.clone());
                self.image_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(ip));

                let mut vp = MockVideoProvider::new();
                vp.configure(config);
                self.video_providers
                    .write()
                    .await
                    .insert(name.to_string(), Arc::new(vp));
                info!(provider = "mock", "Registered mock image+video provider");
            }
            _ => {
                warn!(provider = %name, "Unknown provider, skipping");
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
