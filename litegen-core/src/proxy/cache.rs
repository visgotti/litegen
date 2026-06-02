use moka::future::Cache;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tracing::debug;

use crate::config::CacheGlobalConfig;
use crate::providers::ImageExtras;
use crate::types::{BaseGenerationRequest, ImageGenerationResponse};

/// In-memory generation cache using moka.
/// Caches image generation responses keyed by a hash that covers every input
/// that can affect the output — model, prompt, negative_prompt, seed, size,
/// aspect_ratio, quality, style, steps, guidance_scale, strength, n, and
/// any `extra` provider-specific knobs. Two requests with the same model + prompt
/// but different sizes (or seeds, or anything else) MUST NOT collide.
pub struct GenerationCache {
    image_cache: Option<Cache<String, ImageGenerationResponse>>,
    enabled: bool,
}

impl GenerationCache {
    pub fn new(config: &CacheGlobalConfig) -> Self {
        if !config.enabled {
            return Self {
                image_cache: None,
                enabled: false,
            };
        }

        let cache = Cache::builder()
            .max_capacity(config.max_items)
            .time_to_live(Duration::from_secs(config.default_ttl_seconds))
            .build();

        Self {
            image_cache: Some(cache),
            enabled: true,
        }
    }

    pub fn is_enabled(&self) -> bool {
        self.enabled
    }

    /// Get a cached image generation response. The full request shape is
    /// fingerprinted so requests differing in size/seed/etc don't collide.
    pub async fn get_image(
        &self,
        model: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
    ) -> Option<ImageGenerationResponse> {
        let cache = self.image_cache.as_ref()?;
        let key = image_cache_key(model, base, extras);
        let result = cache.get(&key).await;
        if result.is_some() {
            debug!(model = model, "Cache hit");
        }
        result
    }

    /// Store an image generation response in cache.
    pub async fn put_image(
        &self,
        model: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        response: &ImageGenerationResponse,
    ) {
        if let Some(cache) = &self.image_cache {
            let key = image_cache_key(model, base, extras);
            cache.insert(key, response.clone()).await;
            debug!(model = model, "Cached generation");
        }
    }

    /// Invalidate a specific cache entry.
    pub async fn invalidate(
        &self,
        model: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
    ) {
        if let Some(cache) = &self.image_cache {
            cache.invalidate(&image_cache_key(model, base, extras)).await;
        }
    }

    /// Clear the entire cache.
    pub async fn clear(&self) {
        if let Some(cache) = &self.image_cache {
            cache.invalidate_all();
        }
    }

    /// Get approximate cache entry count.
    pub fn entry_count(&self) -> u64 {
        self.image_cache
            .as_ref()
            .map(|c| c.entry_count())
            .unwrap_or(0)
    }
}

fn image_cache_key(
    model: &str,
    base: &BaseGenerationRequest,
    extras: &ImageExtras,
) -> String {
    let mut hasher = Sha256::new();
    let mut feed = |s: &str| {
        hasher.update(s.as_bytes());
        hasher.update(b"\x00");
    };
    feed(model);
    feed(&base.prompt);
    feed(base.negative_prompt.as_deref().unwrap_or(""));
    feed(&base.seed.map(|v| v.to_string()).unwrap_or_default());
    feed(&base.n.to_string());
    feed(extras.size.as_deref().unwrap_or(""));
    feed(extras.aspect_ratio.as_deref().unwrap_or(""));
    feed(extras.quality.as_deref().unwrap_or(""));
    feed(extras.style.as_deref().unwrap_or(""));
    feed(&extras.steps.map(|v| v.to_string()).unwrap_or_default());
    feed(&extras.guidance_scale.map(|v| v.to_string()).unwrap_or_default());
    feed(&extras.strength.map(|v| v.to_string()).unwrap_or_default());
    feed(&extras.response_format);
    // Reference images affect output but are materialized at call time;
    // we fingerprint the unmaterialized array here (good enough — same input
    // → same materialization).
    feed(&serde_json::to_string(&base.reference_images).unwrap_or_default());
    feed(&extras.extra.as_ref().and_then(|v| serde_json::to_string(v).ok()).unwrap_or_default());
    hex::encode(hasher.finalize())
}
