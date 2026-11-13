use moka::future::Cache;
use moka::Expiry;
use sha2::{Digest, Sha256};
use std::sync::atomic::{AtomicI64, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, OnceLock, RwLock};
use std::time::{Duration, Instant};
use tracing::debug;

use crate::config::CacheGlobalConfig;
use crate::management::settings::CacheSettings;
use crate::providers::ImageExtras;
use crate::proxy::materializer::MaterializedRequest;
pub use crate::shared::cache_tier::MAX_SHARED_ENTRY_BYTES;
use crate::shared::{cache_tier, SharedError, SharedRedis, Throttle};
use crate::types::{BaseGenerationRequest, ImageGenerationResponse};

/// What the cache did for one answer, sent as `X-Litegen-Cache`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CacheOutcome {
    /// Answered from the cache.
    Hit,
    /// Looked up and not found: the vendor answered.
    Miss,
    /// Not looked up: the cache is off for this model, or the request said `no-cache`.
    Bypass,
}

impl CacheOutcome {
    pub fn as_str(self) -> &'static str {
        match self {
            CacheOutcome::Hit => "hit",
            CacheOutcome::Miss => "miss",
            CacheOutcome::Bypass => "bypass",
        }
    }
}

/// What one request lets the cache do, from its `Cache-Control` header, as
/// LiteLLM's `cache` request param: `no-cache` answers fresh (and keeps it),
/// `no-store` keeps nothing; together, the cache is not touched.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CachePolicy {
    pub lookup: bool,
    pub store: bool,
}

impl Default for CachePolicy {
    fn default() -> Self {
        Self { lookup: true, store: true }
    }
}

impl CachePolicy {
    /// Neither looked up nor stored: a request that keeps nothing.
    pub fn bypass() -> Self {
        Self { lookup: false, store: false }
    }

    pub fn from_headers(headers: &axum::http::HeaderMap) -> Self {
        let mut policy = Self::default();
        for value in headers.get_all(axum::http::header::CACHE_CONTROL) {
            for directive in value.to_str().unwrap_or("").split(',') {
                match directive.trim().to_ascii_lowercase().as_str() {
                    "no-cache" => policy.lookup = false,
                    "no-store" => policy.store = false,
                    _ => {}
                }
            }
        }
        policy
    }
}

/// An answer with its own lifetime: its route's TTL, else the gateway's.
#[derive(Clone)]
struct Cached {
    response: ImageGenerationResponse,
    ttl: Duration,
}

struct PerEntryTtl;

impl Expiry<String, Cached> for PerEntryTtl {
    fn expire_after_create(&self, _key: &String, value: &Cached, _created_at: Instant) -> Option<Duration> {
        Some(value.ttl)
    }
}

/// About how many bytes one answer holds in the local tier: the strings by
/// length, the fixed parts by `size_of`. Nothing is serialized, which would
/// copy a `b64_json` image's megabytes.
fn estimated_bytes(key: &str, value: &Cached) -> u64 {
    let r = &value.response;
    let text = |s: &Option<String>| s.as_ref().map_or(0, String::len);
    let images: usize = r
        .data
        .iter()
        .map(|i| {
            std::mem::size_of::<crate::types::ImageResult>()
                + text(&i.url)
                + text(&i.b64_json)
                + text(&i.revised_prompt)
                + text(&i.view)
                + text(&i.direction)
                + i.content_type.len()
        })
        .sum();
    let metadata: usize = r.metadata.as_ref().map_or(0, |m| m.iter().map(|(k, v)| k.len() + v.len()).sum());
    (key.len() + std::mem::size_of::<Cached>() + r.model.len() + r.provider.len() + r.id.len() + images + metadata) as u64
}

/// The local tier, bounded by `max_items` answers and about `max_bytes`
/// bytes through one weigher (P7, code review 2026-11-11). Every answer
/// weighs at least `unit = ceil(max_bytes / max_items)`, and moka's capacity
/// is `max_items × unit`, so:
/// - at most `max_items` answers fit, each weighing at least `unit`;
/// - the answers' bytes stay within `max_items × unit`, which is `max_bytes`
///   rounded up to a whole unit (at most `max_items − 1` bytes over). A
///   weight is a `u32`, so when `max_bytes / max_items` is over `u32::MAX`
///   the unit is clamped, which lowers the byte bound to `max_items × 4 GiB`.
///
/// An answer heavier than the room left gets in only past moka's TinyLFU
/// admission, and one heavier than the whole capacity never. moka applies
/// writes in its housekeeping (after 64 pending writes, or its sync
/// interval), so a burst of large answers can be over the bound until then.
fn build(max_items: u64, max_bytes: u64) -> Cache<String, Cached> {
    let unit = max_bytes.div_ceil(max_items.max(1)).clamp(1, u64::from(u32::MAX));
    Cache::builder()
        .max_capacity(max_items.saturating_mul(unit))
        .weigher(move |key: &String, value: &Cached| -> u32 {
            estimated_bytes(key, value).clamp(unit, u64::from(u32::MAX)) as u32
        })
        .expire_after(PerEntryTtl)
        .build()
}

/// The generation cache: moka in memory on each replica, and with Redis a
/// shared tier behind it (LiteLLM's `DualCache`).
/// Caches image generation responses keyed by a hash that covers every input
/// that can affect the output — model, prompt, negative_prompt, seed, size,
/// aspect_ratio, quality, style, steps, guidance_scale, strength, n, and
/// any `extra` provider-specific knobs. Two requests with the same model + prompt
/// but different sizes (or seeds, or anything else) MUST NOT collide.
///
/// Whether an answer is cached is the gateway's settings' call, and a route's
/// (`docs/gateway-settings.md`); this is the storage, sized to the settings.
///
/// **The two tiers** (`docs/scaling.md`). The local tier is this replica's
/// memory: `cache_max_items` and `cache_max_bytes` cap it (whichever is
/// reached first evicts, see [`build`]) and `entry_count` counts it. With
/// Redis attached ([`Self::attach_shared`]):
///
/// - a local miss reads the shared tier, and a hit is copied into the local
///   one with the life it has left in Redis;
/// - a store writes both, except an answer over [`MAX_SHARED_ENTRY_BYTES`],
///   which stays local;
/// - [`Self::clear`] empties both tiers and raises a shared epoch, and every
///   other replica, told by an event or at its next refresh, empties its own
///   local tier when it sees the epoch move ([`Self::sync_epoch`]);
/// - Redis never blocks and never refuses: an error on the shared tier is a
///   miss on a lookup, and a skipped write on a store.
pub struct GenerationCache {
    image_cache: RwLock<Cache<String, Cached>>,
    /// The `(max_items, max_bytes)` the local tier was built with. One lock
    /// for the pair, so two settings changes at once can't leave it out of
    /// step with the tier that was built.
    capacity: std::sync::Mutex<(u64, u64)>,
    /// The shared tier. Set once at boot, if there is a Redis.
    shared: OnceLock<Arc<SharedRedis>>,
    /// The last shared epoch this replica emptied its local tier for, or made
    /// itself by clearing.
    seen_epoch: AtomicI64,
    /// How many times the local tier has been emptied (`clear_local`). A
    /// lookup that read an answer from Redis compares it before and after it
    /// keeps the answer, to learn that a clear ran in between.
    local_gen: AtomicU64,
    /// Writes to the shared tier that are still on their way (see `put_image`).
    shared_writes: Arc<AtomicUsize>,
    /// Test only: stops the next shared lookup between Redis's reply and
    /// keeping the answer, so a test can run a clear at that very point.
    #[cfg(test)]
    lookup_pause: std::sync::Mutex<Option<Arc<LookupPause>>>,
    /// Test only: stops the next `clear` after it emptied the local tier and
    /// before it touches the shared one, so a test can drop its caller there.
    #[cfg(test)]
    clear_pause: std::sync::Mutex<Option<Arc<LookupPause>>>,
}

/// See [`GenerationCache::pause_next_shared_lookup`] and
/// [`GenerationCache::pause_next_clear`].
#[cfg(test)]
pub(crate) struct LookupPause {
    /// Notified when the paused call has reached its stop and waits.
    pub reached: tokio::sync::Notify,
    /// Notify it to let the paused call go on.
    pub resume: tokio::sync::Notify,
}

/// One write to the shared tier in flight; it counts itself out when dropped,
/// whether the write finished, failed or was cancelled.
struct SharedWrite(Arc<AtomicUsize>);

impl SharedWrite {
    fn begin(count: &Arc<AtomicUsize>) -> Self {
        count.fetch_add(1, Ordering::SeqCst);
        Self(count.clone())
    }
}

impl Drop for SharedWrite {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::SeqCst);
    }
}

/// One log line a minute for each way the shared tier can fail: the cache
/// goes on without it, and an outage would otherwise write a line per request.
static LOOKUP_FAILED: Throttle = Throttle::new();
static STORE_FAILED: Throttle = Throttle::new();
static CLEAR_FAILED: Throttle = Throttle::new();
static EPOCH_FAILED: Throttle = Throttle::new();

/// Say, at most once a minute per `throttle`, that the shared tier failed and
/// what the cache did instead. `error` is Redis's or the network's own text,
/// never the URL.
fn warn_shared(throttle: &Throttle, error: &SharedError, consequence: &str) {
    if throttle.due(Instant::now()) {
        tracing::warn!(%error, "the shared cache tier failed; {consequence}");
    }
}

impl GenerationCache {
    pub fn new(config: &CacheGlobalConfig) -> Self {
        Self {
            image_cache: RwLock::new(build(config.max_items, config.max_bytes)),
            capacity: std::sync::Mutex::new((config.max_items, config.max_bytes)),
            shared: OnceLock::new(),
            seen_epoch: AtomicI64::new(0),
            local_gen: AtomicU64::new(0),
            shared_writes: Arc::new(AtomicUsize::new(0)),
            #[cfg(test)]
            lookup_pause: std::sync::Mutex::new(None),
            #[cfg(test)]
            clear_pause: std::sync::Mutex::new(None),
        }
    }

    /// Wait until every write to the shared tier that `put_image` started has
    /// finished, one way or another. For tests, and for a replica that wants
    /// its answers in Redis before it stops.
    pub async fn flush_shared_writes(&self) {
        while self.shared_writes.load(Ordering::SeqCst) > 0 {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    }

    /// Test only: the next shared lookup stops after Redis has answered and
    /// before it keeps the answer, and says so on `reached`.
    #[cfg(test)]
    pub(crate) fn pause_next_shared_lookup(&self) -> Arc<LookupPause> {
        let pause = Arc::new(LookupPause { reached: tokio::sync::Notify::new(), resume: tokio::sync::Notify::new() });
        *self.lookup_pause.lock().unwrap() = Some(pause.clone());
        pause
    }

    #[cfg(test)]
    async fn pause_after_shared_read(&self) {
        let pause = self.lookup_pause.lock().unwrap().take();
        if let Some(pause) = pause {
            pause.reached.notify_one();
            pause.resume.notified().await;
        }
    }

    /// Test only: the next `clear` stops once this replica's tier is empty and
    /// before it touches Redis, and says so on `reached`.
    #[cfg(test)]
    pub(crate) fn pause_next_clear(&self) -> Arc<LookupPause> {
        let pause = Arc::new(LookupPause { reached: tokio::sync::Notify::new(), resume: tokio::sync::Notify::new() });
        *self.clear_pause.lock().unwrap() = Some(pause.clone());
        pause
    }

    #[cfg(test)]
    async fn pause_after_local_clear(&self) {
        let pause = self.clear_pause.lock().unwrap().take();
        if let Some(pause) = pause {
            pause.reached.notify_one();
            pause.resume.notified().await;
        }
    }

    /// Note the epoch this replica's own clear made, unless a sync meanwhile
    /// has seen a later one: the seen epoch only moves forward here.
    fn record_cleared(&self, epoch: i64) {
        self.seen_epoch.fetch_max(epoch, Ordering::SeqCst);
    }

    /// Put a shared tier behind the local one. Called once, at boot, by
    /// `ProxyRouter::with_shared`; a second call changes nothing.
    pub fn attach_shared(&self, shared: Arc<SharedRedis>) {
        let _ = self.shared.set(shared);
    }

    /// Empty this replica's tier only.
    ///
    /// The count goes up first, then the tier is emptied. A lookup that is in
    /// the middle of keeping an answer it read from Redis reads the count
    /// before and after ([`Self::shared_lookup`]): it either sees the count
    /// move, and drops the answer, or it kept the answer before the tier was
    /// emptied, and the emptying drops it.
    pub async fn clear_local(&self) {
        self.local_gen.fetch_add(1, Ordering::SeqCst);
        self.current().invalidate_all();
    }

    /// Empty the local tier if another replica cleared the cache since this
    /// one last looked: the shared epoch is not the one seen. Without a shared
    /// tier, or when Redis cannot be read, it does nothing (the next call, at
    /// the next refresh, tries again).
    pub async fn sync_epoch(&self) {
        let Some(shared) = self.shared.get() else { return };
        match cache_tier::epoch(shared).await {
            Ok(epoch) => {
                if self.seen_epoch.swap(epoch, Ordering::SeqCst) != epoch {
                    self.clear_local().await;
                }
            }
            Err(e) => warn_shared(&EPOCH_FAILED, &e, "could not check whether another replica cleared the cache, so this one keeps its answers until it can"),
        }
    }

    fn current(&self) -> Cache<String, Cached> {
        self.image_cache.read().unwrap_or_else(|p| p.into_inner()).clone()
    }

    /// Follow a change of settings: turned off, the cache is emptied; given
    /// a new size (`max_items` or `max_bytes`), it starts again empty at that
    /// size. A new TTL applies to what is stored from now on. This replica's
    /// local tier only: whoever turns the cache off also clears the shared
    /// tier (`put_settings`), and the shared tier has no size to change.
    pub fn reconfigure(&self, settings: CacheSettings) {
        let size = (settings.max_items, settings.max_bytes);
        let mut capacity = self.capacity.lock().unwrap_or_else(|p| p.into_inner());
        let resized = std::mem::replace(&mut *capacity, size) != size;
        if resized || !settings.enabled {
            *self.image_cache.write().unwrap_or_else(|p| p.into_inner()) = build(settings.max_items, settings.max_bytes);
        }
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
        let key = image_cache_key(tenant, model, base, extras, materialized);
        let local = self.current();
        let mut result = local.get(&key).await.map(|c| c.response);
        if result.is_none() {
            result = self.shared_lookup(&local, &key).await;
        }
        let outcome = if result.is_some() { CacheOutcome::Hit } else { CacheOutcome::Miss };
        metrics::counter!("litegen_cache_requests_total", "result" => outcome.as_str()).increment(1);
        if result.is_some() {
            debug!(model = model, "Cache hit");
        }
        result
    }

    /// A local miss's second chance: the shared tier. A hit is copied into
    /// `local` with the life Redis says it has left, so this replica never
    /// keeps an answer longer than the replica that cached it meant to. An
    /// error is a miss.
    ///
    /// **A clear can overtake the lookup.** Redis answers, then the cache is
    /// cleared (here, or on another replica whose epoch this one syncs), and
    /// only then is the answer kept: it would outlive the clear for its whole
    /// TTL. So the count of local clears is read before Redis is asked, and
    /// again after the answer is kept; if it moved, the answer is dropped
    /// again. The lookup still returns what it read: it was a hit when Redis
    /// answered. (A clear that comes after the second read empties the tier
    /// after the answer is in it, so that one drops it.)
    async fn shared_lookup(&self, local: &Cache<String, Cached>, key: &str) -> Option<ImageGenerationResponse> {
        let shared = self.shared.get()?;
        let generation = self.local_gen.load(Ordering::SeqCst);
        match cache_tier::get(shared, key).await {
            Ok(Some((response, ttl))) => {
                #[cfg(test)]
                self.pause_after_shared_read().await;
                local.insert(key.to_string(), Cached { response: response.clone(), ttl }).await;
                if self.local_gen.load(Ordering::SeqCst) != generation {
                    local.invalidate(key).await;
                }
                Some(response)
            }
            Ok(None) => None,
            Err(e) => {
                warn_shared(&LOOKUP_FAILED, &e, "answers are read from this replica's own tier only until it works again");
                None
            }
        }
    }

    /// Store an image generation response for `ttl`.
    #[allow(clippy::too_many_arguments)]
    pub async fn put_image(
        &self,
        tenant: Option<&str>,
        model: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
        response: &ImageGenerationResponse,
        ttl: Duration,
    ) {
        let key = image_cache_key(tenant, model, base, extras, materialized);
        self.current().insert(key.clone(), Cached { response: response.clone(), ttl }).await;
        // The write to Redis goes on in the background: a slow Redis never
        // delays the response that is waiting on this. An answer that cannot
        // fit is not even copied. A failed write is logged (once a minute) and
        // leaves the answer on this replica only.
        if let Some(shared) = self.shared.get().filter(|_| !cache_tier::too_big_to_share(response)) {
            let (shared, response) = (shared.clone(), response.clone());
            let write = SharedWrite::begin(&self.shared_writes);
            tokio::spawn(async move {
                let _write = write;
                if let Err(e) = cache_tier::put(&shared, &key, &response, ttl).await {
                    warn_shared(&STORE_FAILED, &e, "this answer is kept on this replica only");
                }
            });
        }
        debug!(model = model, "Cached generation");
    }

    /// Invalidate a specific cache entry, in both tiers. Other replicas keep
    /// their own copy of it until it expires or the cache is cleared.
    pub async fn invalidate(
        &self,
        tenant: Option<&str>,
        model: &str,
        base: &BaseGenerationRequest,
        extras: &ImageExtras,
        materialized: &MaterializedRequest,
    ) {
        let key = image_cache_key(tenant, model, base, extras, materialized);
        self.current().invalidate(&key).await;
        if let Some(shared) = self.shared.get() {
            if let Err(e) = cache_tier::remove(shared, &key).await {
                warn_shared(&STORE_FAILED, &e, "the answer stays in the shared tier until it expires");
            }
        }
    }

    /// Clear the entire cache, on every replica: this one's local tier, then the
    /// shared answers, then the shared epoch goes up, which tells every other
    /// replica to empty its local tier ([`Self::sync_epoch`]).
    ///
    /// This replica notes the epoch its own clear made, so its next
    /// `sync_epoch` does not empty a tier that already holds newer answers.
    ///
    /// When Redis fails the local tier is still cleared and the failure is
    /// logged (once a minute) and returned, but the epoch is not raised: the
    /// other replicas are not told, and keep what they hold in memory until
    /// it expires or a clear that finishes. The caller says so: `DELETE
    /// /v1/cache` answers 503 `cache_clear_incomplete`. `Ok` without a shared
    /// tier, or when every page and the epoch went through.
    pub async fn clear(&self) -> Result<(), SharedError> {
        self.clear_local().await;
        #[cfg(test)]
        self.pause_after_local_clear().await;
        let Some(shared) = self.shared.get() else { return Ok(()) };
        let cleared = match cache_tier::clear(shared).await {
            Ok(epoch) => {
                self.record_cleared(epoch);
                Ok(())
            }
            Err(e) => {
                warn_shared(&CLEAR_FAILED, &e, "only this replica's tier was cleared, and the other replicas were not told to clear theirs");
                Err(e)
            }
        };
        // Once more: a lookup that read an answer before the shared tier was
        // cleared may be keeping it now, after the first emptying.
        self.clear_local().await;
        cleared
    }

    /// How many answers the cache holds. moka counts lazily, so its pending
    /// housekeeping runs first and the count is exact.
    pub async fn entry_count(&self) -> u64 {
        let cache = self.current();
        cache.run_pending_tasks().await;
        cache.entry_count()
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
            size: None, resolution: None, aspect_ratio: None, quality: None, style: None, steps: None,
            guidance_scale: None, strength: None, response_format: "url".to_string(),
            edit: None, views: Vec::new(), extra: None, output_format: None, background: None,
            expand: None, layout: None, mask_prompt: None, preserve_unmasked: None, feather_px: None,
            palette: None, directions: None,
        }
    }

    #[test]
    fn a_clear_never_moves_the_seen_epoch_back() {
        let cache = GenerationCache::new(&CacheGlobalConfig::default());
        cache.seen_epoch.store(5, Ordering::SeqCst);
        // A sync that ran meanwhile already saw a later clear's epoch.
        cache.record_cleared(3);
        assert_eq!(cache.seen_epoch.load(Ordering::SeqCst), 5);
        cache.record_cleared(7);
        assert_eq!(cache.seen_epoch.load(Ordering::SeqCst), 7);
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
            ("resolution", ImageExtras { resolution: Some("4K".into()), ..extras() }),
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

    /// An answer whose one image is `b64_bytes` of base64.
    fn answer(id: &str, b64_bytes: usize) -> ImageGenerationResponse {
        serde_json::from_value(json!({
            "created": 1, "model": "m", "provider": "mock", "id": id,
            "data": [{ "b64_json": "A".repeat(b64_bytes), "index": 0 }]
        }))
        .unwrap()
    }

    /// Store `n` answers to distinct prompts.
    async fn put_answers(cache: &GenerationCache, n: usize, b64_bytes: usize) {
        for i in 0..n {
            let base: BaseGenerationRequest = serde_json::from_value(json!({ "prompt": format!("a cat {i}"), "model": "m" })).unwrap();
            let response = answer(&format!("r{i}"), b64_bytes);
            cache.put_image(None, "m", &base, &extras(), &MaterializedRequest::default(), &response, Duration::from_secs(600)).await;
        }
    }

    /// P7 (code review 2026-11-11): the local tier is bounded in bytes as well
    /// as in answers. A `b64_json` answer carries its whole image, so a count
    /// bound alone let 10,000 answers of a few MB take tens of GiB.
    #[tokio::test]
    async fn the_cache_holds_no_more_bytes_than_max_bytes() {
        let cfg: CacheGlobalConfig =
            serde_json::from_value(json!({ "enabled": true, "max_items": 10000, "max_bytes": 4_194_304 })).unwrap();
        let cache = GenerationCache::new(&cfg);
        put_answers(&cache, 16, 1 << 20).await;
        let n = cache.entry_count().await;
        assert!(n <= 4, "the cache kept {n} answers of 1 MiB under a 4 MiB max_bytes");
    }

    /// Guard: the byte budget keeps the count bound. It catches a weigher
    /// that weighs small answers by their bytes alone: twenty of them would
    /// then fit in 256 MiB, far past `max_items`.
    #[tokio::test]
    async fn the_cache_still_holds_no_more_answers_than_max_items() {
        let cfg = CacheGlobalConfig { enabled: true, max_items: 5, max_bytes: 268_435_456, ..CacheGlobalConfig::default() };
        let cache = GenerationCache::new(&cfg);
        put_answers(&cache, 20, 16).await;
        let n = cache.entry_count().await;
        assert!(n <= 5, "the cache kept {n} answers under max_items 5");
    }

    /// Guard: a settings change to `max_bytes` alone rebuilds the tier at the
    /// new budget (a new size is a new, empty cache, as for `max_items`).
    #[tokio::test]
    async fn a_new_max_bytes_rebuilds_the_tier_at_that_budget() {
        let cache = GenerationCache::new(&CacheGlobalConfig { enabled: true, ..CacheGlobalConfig::default() });
        put_answers(&cache, 4, 1 << 20).await;
        assert_eq!(cache.entry_count().await, 4, "256 MiB holds four 1 MiB answers");
        cache.reconfigure(CacheSettings { enabled: true, ttl: Duration::from_secs(600), max_items: 10_000, max_bytes: 2_097_152 });
        assert_eq!(cache.entry_count().await, 0, "a new budget is a new, empty tier");
        put_answers(&cache, 8, 1 << 20).await;
        let n = cache.entry_count().await;
        assert!(n <= 2, "the rebuilt tier kept {n} answers of 1 MiB under 2 MiB");
    }
}
