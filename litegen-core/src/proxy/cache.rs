use moka::future::Cache;
use sha2::{Digest, Sha256};
use std::time::Duration;
use tracing::debug;

use crate::config::CacheGlobalConfig;
use crate::providers::ImageExtras;
use crate::proxy::materializer::MaterializedRequest;
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
        tenant: Option<&str>,
        model: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) -> Option<ImageGenerationResponse> {
        let cache = self.image_cache.as_ref()?;
        let key = image_cache_key(tenant, model, base, extras, materialized);
        let result = cache.get(&key).await;
        if result.is_some() {
            debug!(model = model, "Cache hit");
        }
        result
    }

    /// Store an image generation response in cache.
    pub async fn put_image(
        &self,
        tenant: Option<&str>,
        model: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        response: &ImageGenerationResponse,
    ) {
        if let Some(cache) = &self.image_cache {
            let key = image_cache_key(tenant, model, base, extras, materialized);
            cache.insert(key, response.clone()).await;
            debug!(model = model, "Cached generation");
        }
    }

    /// Invalidate a specific cache entry.
    pub async fn invalidate(
        &self,
        tenant: Option<&str>,
        model: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) {
        if let Some(cache) = &self.image_cache {
            cache.invalidate(&image_cache_key(tenant, model, base, extras, materialized)).await;
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
    tenant: Option<&str>,
    model: &str,
    base: &BaseGenerationRequest,
    extras: &ImageExtras,
    materialized: &MaterializedRequest,
) -> String {
    let mut hasher = Sha256::new();
    let mut feed = |b: &[u8]| {
        hasher.update(b);
        hasher.update(b"\x00");
    };
    // Tenant scope FIRST: the cache is a single process-global instance, so the
    // key must include the caller's tenant (app/org) or one tenant could receive
    // another tenant's cached image for an identical model+prompt+params request.
    feed(tenant.unwrap_or("__global__").as_bytes());
    feed(model.as_bytes());
    feed(base.prompt.as_bytes());
    feed(base.negative_prompt.as_deref().unwrap_or("").as_bytes());
    feed(base.seed.map(|v| v.to_string()).unwrap_or_default().as_bytes());
    feed(base.n.to_string().as_bytes());
    // Every image option as serialised — size and quality, the edit window,
    // layout, palette, views, format, `extra`… — so a field added later is
    // keyed without anyone remembering to add it here.
    feed(serde_json::to_string(extras).unwrap_or_default().as_bytes());
    // Reference images affect output but are materialized at call time;
    // we fingerprint the unmaterialized array here (good enough — same input
    // → same materialization).
    feed(serde_json::to_string(&base.reference_images).unwrap_or_default().as_bytes());
    // Except a mask litegen made itself — drawn from `region`, grown by
    // `mask_grow_px` — which exists only in the materialized request.
    for r in materialized.refs.iter().filter(|r| r.role == "mask") {
        use crate::proxy::materializer::MaterializedRefForm;
        match &r.form {
            MaterializedRefForm::Url(u) | MaterializedRefForm::Base64(u) => feed(u.as_bytes()),
            MaterializedRefForm::MultipartField { bytes, .. } => feed(bytes),
        }
    }
    hex::encode(hasher.finalize())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::proxy::materializer::{MaterializedRef, MaterializedRefForm};
    use serde_json::json;

    fn extras() -> ImageExtras {
        ImageExtras {
            vendor_job: Default::default(),
            size: None, aspect_ratio: None, quality: None, style: None, steps: None,
            guidance_scale: None, strength: None, response_format: "url".to_string(),
            edit: None, views: Vec::new(), extra: None, output_format: None, background: None,
            expand: None, layout: None, mask_prompt: None, preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    fn masked(png: &str) -> MaterializedRequest {
        MaterializedRequest {
            refs: vec![MaterializedRef { role: "mask".into(), form: MaterializedRefForm::Base64(png.into()), time_s: None }],
            ..Default::default()
        }
    }

    /// Two requests that make different images never share a cache entry:
    /// every image option is keyed, and so is a mask litegen drew from
    /// `region` or grew by `mask_grow_px`, which lives only in the
    /// materialized request.
    #[test]
    fn every_field_that_changes_the_image_changes_its_cache_key() {
        let base: BaseGenerationRequest = serde_json::from_value(json!({ "prompt": "a cat", "model": "m" })).unwrap();
        let none = MaterializedRequest::default();
        let key = |e: &ImageExtras, m: &MaterializedRequest| image_cache_key(None, "m", &base, e, m);
        let plain = key(&extras(), &none);
        let window = crate::types::Region { x: 0, y: 0, w: 256, h: 256 };
        let variants = vec![
            ("expand", ImageExtras { expand: Some(crate::types::Expand { left: 64, ..Default::default() }), ..extras() }),
            ("layout", ImageExtras { layout: Some(serde_json::from_value(json!([{ "box": { "x": 0, "y": 0, "w": 10, "h": 10 }, "kind": "object", "prompt": "a hat" }])).unwrap()), ..extras() }),
            ("mask_prompt", ImageExtras { mask_prompt: Some("the sky".into()), ..extras() }),
            ("preserve_unmasked", ImageExtras { preserve_unmasked: Some(true), ..extras() }),
            ("feather_px", ImageExtras { feather_px: Some(4), ..extras() }),
            ("palette", ImageExtras { palette: Some(vec!["#000000".into()]), ..extras() }),
            ("directions", ImageExtras { directions: Some(8), ..extras() }),
            ("views", ImageExtras { views: vec!["front".into()], ..extras() }),
            ("output_format", ImageExtras { output_format: Some("webp".into()), ..extras() }),
            ("background", ImageExtras { background: Some("transparent".into()), ..extras() }),
            ("edit", ImageExtras { edit: Some(crate::providers::ImageEdit { window: Some(window), ..Default::default() }), ..extras() }),
        ];
        for (field, e) in &variants {
            assert_ne!(key(e, &none), plain, "{field} must change the key");
        }
        assert_eq!(key(&extras(), &none), plain, "the same request keys the same");
        assert_ne!(key(&extras(), &masked("AAAA")), plain, "a drawn mask changes the key");
        assert_ne!(key(&extras(), &masked("AAAA")), key(&extras(), &masked("BBBB")), "a mask from another region or growth");
    }
}
