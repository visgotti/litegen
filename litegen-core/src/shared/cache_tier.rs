//! The shared tier of the image cache: what `proxy::cache::GenerationCache`
//! keeps in Redis behind each replica's own memory, so an answer cached on one
//! replica is a hit on the others (`docs/scaling.md`).
//!
//! Keys, under the prefix (`litegen:`):
//!
//! - `cache:e:<sha>`: one answer, the sha being the cache key the local tier
//!   uses. The value is `{"response": <the answer>, "expires_ms": <n>}` and the
//!   key expires with `PX`, so Redis drops it when the answer's TTL is up.
//! - `cache:epoch`: a counter that goes up by one at every clear. A replica
//!   that sees a number it has not seen empties its local tier
//!   (`GenerationCache::sync_epoch`).
//!
//! Two rules keep the replicas honest, the same two as `rate_limit`:
//!
//! - **One step.** An answer is read together with its remaining life (`GET`
//!   and `PTTL` in one script), and written together with its expiry. Redis
//!   runs a script without interleaving another command.
//! - **One clock.** `expires_ms` is Redis's `TIME` plus the TTL, never a
//!   timestamp a replica sends, and the remaining life of a hit is Redis's
//!   `PTTL`. Nothing compares a stored time with a replica's own clock, so a
//!   replica whose clock is off cannot keep an answer too long or drop it too
//!   soon. `expires_ms` is written for whoever looks at the key; the cache
//!   never reads it back.
//!
//! The functions here report a [`SharedError`] and decide nothing: the cache
//! treats any error as a miss (or skips the write) and never refuses a
//! request because of Redis.

use std::sync::LazyLock;
use std::time::{Duration, Instant};

use serde::Deserialize;

use super::{SharedError, SharedRedis, Throttle};
use crate::types::ImageGenerationResponse;

/// The largest entry the shared tier takes, in bytes of the stored JSON: 256
/// KiB. An answer with `b64_json` can be megabytes, and Redis would fill with
/// it. A bigger answer stays on the replica that made it.
pub const MAX_SHARED_ENTRY_BYTES: usize = 262_144;

/// What the stored value adds to the answer's JSON: `{"response":`, then
/// `,"expires_ms":`, then a 13-digit millisecond time (until the year 2286),
/// then `}`. [`PUT_SCRIPT`] writes exactly this around the answer.
const ENVELOPE_BYTES: usize = r#"{"response":"#.len() + r#","expires_ms":"#.len() + 13 + "}".len();

/// Stores the answer in `ARGV[1]` (its JSON) under `KEYS[1]` for `ARGV[2]`
/// milliseconds. Returns `expires_ms`, which is Redis's clock plus the TTL.
///
/// The answer's JSON goes into the envelope as text, untouched, so a byte
/// the answer holds stays as it is. `PX` makes Redis drop the key when the
/// TTL is up.
pub const PUT_SCRIPT: &str = r#"
local t = redis.call('TIME')
local expires_ms = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000) + tonumber(ARGV[2])
local entry = '{"response":' .. ARGV[1] .. ',"expires_ms":' .. string.format('%.0f', expires_ms) .. '}'
redis.call('SET', KEYS[1], entry, 'PX', ARGV[2])
return string.format('%.0f', expires_ms)
"#;

/// Reads the answer in `KEYS[1]` together with its remaining life. Returns
/// `{value, pttl}`, or nil when there is no such key. Read in one script, the
/// two cannot disagree: the key cannot expire between them.
pub const GET_SCRIPT: &str = r#"
local value = redis.call('GET', KEYS[1])
if not value then
  return false
end
return {value, redis.call('PTTL', KEYS[1])}
"#;

/// Built once, so each SHA-1 is computed once. Sent by hash, and loaded again
/// when Redis answers `NOSCRIPT` (see `rate_limit`).
static PUT: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(PUT_SCRIPT));
static GET: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(GET_SCRIPT));

/// Whether the answer is too big to share, judged without serializing it: its
/// `b64_json` images alone pass [`MAX_SHARED_ENTRY_BYTES`], so the stored value
/// cannot fit whatever else it holds. An answer that passes this can still be
/// over the cap once its JSON is counted (see [`put`]).
pub fn too_big_to_share(response: &ImageGenerationResponse) -> bool {
    let images: usize = response.data.iter().filter_map(|image| image.b64_json.as_ref()).map(String::len).sum();
    images > MAX_SHARED_ENTRY_BYTES
}

/// One line a minute about entries that cannot be read.
static UNREADABLE: Throttle = Throttle::new();

/// What is stored under `cache:e:<sha>`, as far as a reader cares.
#[derive(Deserialize)]
struct Entry {
    response: ImageGenerationResponse,
}

/// The answer cached under `sha` and how long it has left, or `None` when
/// there is none (never stored, expired, or cleared). So is an entry this
/// version cannot read, for example one written by another version of litegen
/// during a rolling deploy: that is a miss, logged once a minute in its own
/// words, and not Redis failing. The next store of the answer replaces it.
pub async fn get(shared: &SharedRedis, sha: &str) -> Result<Option<(ImageGenerationResponse, Duration)>, SharedError> {
    let key = entry_key(shared, sha);
    let found: Option<(Vec<u8>, i64)> = shared
        .run(|mut connection| async move {
            let mut invocation = GET.prepare_invoke();
            invocation.key(key.as_str());
            invocation.invoke_async(&mut connection).await
        })
        .await?;
    let Some((value, left_ms)) = found else { return Ok(None) };
    // No life left, or none set: not an entry the cache stored.
    let Ok(left_ms) = u64::try_from(left_ms) else { return Ok(None) };
    if left_ms == 0 {
        return Ok(None);
    }
    let entry: Entry = match serde_json::from_slice(&value) {
        Ok(entry) => entry,
        Err(e) => {
            warn_unreadable(&UNREADABLE, &e);
            return Ok(None);
        }
    };
    Ok(Some((entry.response, Duration::from_millis(left_ms))))
}

/// Say, through `throttle`, that an entry could not be read. serde's own text
/// is left out: for a type mismatch it quotes the value it met, which here is
/// part of a cached answer (a signed image URL, say). The error's category,
/// line and column say what kind of damage it was and where.
fn warn_unreadable(throttle: &Throttle, error: &serde_json::Error) {
    if throttle.due(Instant::now()) {
        tracing::warn!(
            category = ?error.classify(),
            line = error.line(),
            column = error.column(),
            "a shared cache entry could not be read; treated as a miss"
        );
    }
}

/// Store `response` under `sha` for `ttl`. An answer whose stored JSON would
/// pass [`MAX_SHARED_ENTRY_BYTES`] is not sent: it stays local, which is not an
/// error. A TTL under a millisecond stores nothing either, since `PX 0` is
/// refused and the answer would be gone at once.
pub async fn put(shared: &SharedRedis, sha: &str, response: &ImageGenerationResponse, ttl: Duration) -> Result<(), SharedError> {
    let Ok(ttl_ms) = u64::try_from(ttl.as_millis()) else { return Ok(()) };
    if ttl_ms == 0 || too_big_to_share(response) {
        return Ok(());
    }
    let json = serde_json::to_string(response).map_err(|e| SharedError::Redis(format!("the answer could not be serialized: {e}")))?;
    if json.len() + ENVELOPE_BYTES > MAX_SHARED_ENTRY_BYTES {
        tracing::debug!(bytes = json.len(), "an answer too large for the shared cache stays on this replica");
        return Ok(());
    }
    let key = entry_key(shared, sha);
    let _expires_ms: String = shared
        .run(|mut connection| async move {
            let mut invocation = PUT.prepare_invoke();
            invocation.key(key.as_str()).arg(json).arg(ttl_ms);
            invocation.invoke_async(&mut connection).await
        })
        .await?;
    Ok(())
}

/// Drop the answer under `sha`, if any.
pub async fn remove(shared: &SharedRedis, sha: &str) -> Result<(), SharedError> {
    let key = entry_key(shared, sha);
    shared
        .run(|mut connection| async move { redis::cmd("UNLINK").arg(key).query_async::<i64>(&mut connection).await })
        .await
        .map(|_| ())
}

/// Delete every shared answer, then raise the epoch. Returns the new epoch,
/// which the caller records as seen so it does not empty its own memory again.
///
/// The answers go a page at a time (`SCAN`, then `UNLINK` what the page
/// found), each page a call of its own, so no one call runs long on a big
/// keyspace and an error stops the loop where it is. The epoch is raised only
/// once every page is done: after an error it is left alone, so the other
/// replicas are not told to empty their memory for a clear that did not finish.
pub async fn clear(shared: &SharedRedis) -> Result<i64, SharedError> {
    let pattern = shared.key(&["cache", "e", "*"]);
    let mut cursor = 0u64;
    loop {
        let pattern = pattern.clone();
        let next = shared
            .run(|mut connection| async move {
                let (next, keys): (u64, Vec<String>) = redis::cmd("SCAN")
                    .arg(cursor)
                    .arg("MATCH")
                    .arg(pattern)
                    .arg("COUNT")
                    .arg(500)
                    .query_async(&mut connection)
                    .await?;
                if !keys.is_empty() {
                    redis::cmd("UNLINK").arg(keys).query_async::<i64>(&mut connection).await?;
                }
                Ok(next)
            })
            .await?;
        if next == 0 {
            break;
        }
        cursor = next;
    }
    let key = epoch_key(shared);
    shared
        .run(|mut connection| async move { redis::cmd("INCR").arg(key).query_async::<i64>(&mut connection).await })
        .await
}

/// The epoch: how many clears the shared tier has had. A missing key is 0.
pub async fn epoch(shared: &SharedRedis) -> Result<i64, SharedError> {
    let key = epoch_key(shared);
    let epoch: Option<i64> = shared
        .run(|mut connection| async move { redis::cmd("GET").arg(key).query_async(&mut connection).await })
        .await?;
    Ok(epoch.unwrap_or(0))
}

fn entry_key(shared: &SharedRedis, sha: &str) -> String {
    shared.key(&["cache", "e", sha])
}

fn epoch_key(shared: &SharedRedis) -> String {
    shared.key(&["cache", "epoch"])
}

#[cfg(test)]
pub(crate) mod tests {
    use std::sync::Arc;

    use serde_json::json;

    use super::*;
    use crate::config::CacheGlobalConfig;
    use crate::providers::ImageExtras;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::materializer::MaterializedRequest;
    use crate::config::connections::RedisSettings;
    use crate::shared::testing::{dead_redis, test_prefix, two_shared, CutProxy};
    use crate::types::BaseGenerationRequest;

    // The Redis ones need `LITEGEN_REDIS_TEST_URL` and skip themselves without
    // it; the ones on `dead_redis` need no container.

    const MINUTE: Duration = Duration::from_secs(60);

    /// A replica's cache over `shared`.
    fn cache_with(shared: Arc<SharedRedis>) -> GenerationCache {
        let cache = GenerationCache::new(&CacheGlobalConfig { enabled: true, ..CacheGlobalConfig::default() });
        cache.attach_shared(shared);
        cache
    }

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

    fn base(prompt: &str) -> BaseGenerationRequest {
        serde_json::from_value(json!({ "prompt": prompt, "model": "m" })).unwrap()
    }

    /// Cache `response` for the request with this prompt (and nothing else set).
    pub(crate) async fn cache_put(cache: &GenerationCache, prompt: &str, response: ImageGenerationResponse, ttl: Duration) {
        cache.put_image(None, "m", &base(prompt), &extras(), &MaterializedRequest::default(), &response, ttl).await;
        // The write to Redis goes on in the background; a test that reads it
        // from another replica next waits for it.
        cache.flush_shared_writes().await;
    }

    pub(crate) async fn cache_get(cache: &GenerationCache, prompt: &str) -> Option<ImageGenerationResponse> {
        cache.get_image(None, "m", &base(prompt), &extras(), &MaterializedRequest::default()).await
    }

    async fn cache_invalidate(cache: &GenerationCache, prompt: &str) {
        cache.invalidate(None, "m", &base(prompt), &extras(), &MaterializedRequest::default()).await;
    }

    pub(crate) fn small_response() -> ImageGenerationResponse {
        serde_json::from_value(json!({
            "created": 1, "model": "m", "provider": "mock", "id": "gen-1",
            "data": [{ "url": "https://img.example/a.png", "content_type": "image/png", "index": 0 }],
        }))
        .unwrap()
    }

    /// An answer whose `b64_json` is `len` bytes long.
    fn response_with_b64(len: usize) -> ImageGenerationResponse {
        let mut response = small_response();
        response.data[0].url = None;
        response.data[0].b64_json = Some("A".repeat(len));
        response
    }

    /// The keys under this client's prefix that hold answers.
    pub(crate) async fn entry_keys(shared: &SharedRedis) -> Vec<String> {
        let pattern = shared.key(&["cache", "e", "*"]);
        shared.run(|mut c| async move { redis::cmd("KEYS").arg(pattern).query_async(&mut c).await }).await.unwrap()
    }

    #[tokio::test]
    async fn an_answer_cached_on_one_replica_is_a_hit_on_another() {
        let Some((sa, sb)) = two_shared().await else { return };
        let (a, b) = (cache_with(sa), cache_with(sb));
        cache_put(&a, "prompt-1", small_response(), MINUTE).await;
        assert_eq!(cache_get(&b, "prompt-1").await.map(|r| r.id), Some("gen-1".to_string()));
        assert_eq!(b.entry_count().await, 1, "copied into B's local tier");
    }

    #[tokio::test]
    async fn clear_on_one_replica_clears_the_other() {
        let Some((sa, sb)) = two_shared().await else { return };
        let (a, b) = (cache_with(sa), cache_with(sb));
        cache_put(&a, "p", small_response(), MINUTE).await;
        assert!(cache_get(&b, "p").await.is_some());
        a.clear().await.unwrap();
        b.sync_epoch().await;
        assert!(cache_get(&b, "p").await.is_none());
        assert_eq!(b.entry_count().await, 0);
        assert!(cache_get(&a, "p").await.is_none(), "and A's own tier");
    }

    #[tokio::test]
    async fn oversized_answers_stay_local() {
        let Some((sa, sb)) = two_shared().await else { return };
        let (a, b) = (cache_with(sa.clone()), cache_with(sb));
        cache_put(&a, "big", response_with_b64(MAX_SHARED_ENTRY_BYTES + 1), MINUTE).await;
        assert!(cache_get(&a, "big").await.is_some());
        assert!(cache_get(&b, "big").await.is_none());
        assert!(entry_keys(&sa).await.is_empty(), "nothing was sent to Redis");
    }

    #[tokio::test]
    async fn a_dead_redis_is_a_miss_not_an_error() {
        let c = cache_with(dead_redis(false).await);
        assert!(cache_get(&c, "nothing").await.is_none());
        cache_put(&c, "x", small_response(), MINUTE).await;
        assert!(cache_get(&c, "x").await.is_some(), "local still works");
    }

    #[tokio::test]
    async fn a_missing_epoch_reads_as_zero() {
        let Some((s, _)) = two_shared().await else { return };
        assert_eq!(epoch(&s).await.unwrap(), 0);
    }

    #[tokio::test]
    async fn the_replica_that_clears_does_not_wipe_what_it_cached_after() {
        let Some((sa, sb)) = two_shared().await else { return };
        let (a, b) = (cache_with(sa.clone()), cache_with(sb));
        cache_put(&a, "before", small_response(), MINUTE).await;
        assert!(cache_get(&b, "before").await.is_some(), "B holds it locally too");

        a.clear().await.unwrap();
        assert_eq!(epoch(&sa).await.unwrap(), 1);
        cache_put(&a, "after", small_response(), MINUTE).await;
        a.sync_epoch().await;
        // Counted first: a lookup would fetch a wiped answer back from Redis.
        assert_eq!(a.entry_count().await, 1, "A recorded the epoch its own clear made, so it kept its newer answer");
        assert!(cache_get(&a, "after").await.is_some());

        b.sync_epoch().await;
        assert_eq!(b.entry_count().await, 0, "B, which did not clear, empties its tier");
        assert!(cache_get(&b, "before").await.is_none());
        assert!(cache_get(&b, "after").await.is_some(), "what A cached after the clear is shared");
        b.sync_epoch().await;
        assert_eq!(b.entry_count().await, 1, "the same epoch twice empties nothing");
    }

    #[tokio::test]
    async fn a_shared_hit_is_copied_with_the_remaining_ttl_not_the_full_one() {
        let Some((sa, sb)) = two_shared().await else { return };
        let (a, b) = (cache_with(sa), cache_with(sb));
        cache_put(&a, "short", small_response(), Duration::from_secs(4)).await;
        let stored_at = Instant::now();
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(cache_get(&b, "short").await.is_some(), "three seconds left: a stall of up to two still passes");
        // 4.5 s after the write: Redis dropped it at 4 s, and B's copy had only
        // the three seconds that remained when B took it. With the full 4 s it
        // would live to 5 s. This waits until that time, however long the
        // test thread was stalled before.
        tokio::time::sleep((stored_at + Duration::from_millis(4500)).saturating_duration_since(Instant::now())).await;
        assert!(cache_get(&b, "short").await.is_none());
    }

    #[tokio::test]
    async fn the_remaining_ttl_comes_from_redis() {
        let Some((sa, sb)) = two_shared().await else { return };
        put(&sa, "ttl", &small_response(), MINUTE).await.unwrap();
        tokio::time::sleep(Duration::from_millis(300)).await;
        let (response, left) = get(&sb, "ttl").await.unwrap().expect("stored");
        assert_eq!(response.id, "gen-1");
        assert!(left <= Duration::from_millis(59_700) && left > Duration::from_secs(58), "{left:?}");
        assert!(get(&sb, "other").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn the_stored_entry_is_json_with_an_expiry_from_the_redis_clock() {
        let Some((sa, _)) = two_shared().await else { return };
        let response = small_response();
        put(&sa, "abc", &response, Duration::from_secs(30)).await.unwrap();
        let key = sa.key(&["cache", "e", "abc"]);
        let (stored, pttl, now, strlen): (String, i64, (i64, i64), usize) = sa
            .run(|mut c| async move {
                let stored = redis::cmd("GET").arg(&key).query_async(&mut c).await?;
                let pttl = redis::cmd("PTTL").arg(&key).query_async(&mut c).await?;
                let now = redis::cmd("TIME").query_async(&mut c).await?;
                let strlen = redis::cmd("STRLEN").arg(&key).query_async(&mut c).await?;
                Ok((stored, pttl, now, strlen))
            })
            .await
            .unwrap();
        let value: serde_json::Value = serde_json::from_str(&stored).unwrap();
        assert_eq!(value["response"], serde_json::to_value(&response).unwrap());
        let now_ms = now.0 * 1000 + now.1 / 1000;
        let expires_ms = value["expires_ms"].as_i64().expect("a number");
        assert!((now_ms + 29_000..=now_ms + 30_100).contains(&expires_ms), "{expires_ms} against {now_ms}");
        assert!(pttl > 29_000 && pttl <= 30_000, "PX is the TTL: {pttl}");
        assert_eq!(strlen, serde_json::to_string(&response).unwrap().len() + ENVELOPE_BYTES, "the size rule counts what is stored");
    }

    #[tokio::test]
    async fn the_size_cap_counts_the_whole_stored_entry() {
        let Some((sa, _)) = two_shared().await else { return };
        let empty = serde_json::to_string(&response_with_b64(0)).unwrap().len();
        let room = MAX_SHARED_ENTRY_BYTES - ENVELOPE_BYTES - empty;
        put(&sa, "fits", &response_with_b64(room), MINUTE).await.unwrap();
        put(&sa, "over", &response_with_b64(room + 1), MINUTE).await.unwrap();
        let (fits_key, over_key) = (sa.key(&["cache", "e", "fits"]), sa.key(&["cache", "e", "over"]));
        let (fits, over): (usize, usize) = sa
            .run(|mut c| async move {
                let fits = redis::cmd("STRLEN").arg(fits_key).query_async(&mut c).await?;
                let over = redis::cmd("EXISTS").arg(over_key).query_async(&mut c).await?;
                Ok((fits, over))
            })
            .await
            .unwrap();
        assert_eq!(fits, MAX_SHARED_ENTRY_BYTES, "exactly the cap is taken");
        assert_eq!(over, 0, "one byte more is not");
    }

    #[tokio::test]
    async fn an_unreadable_entry_is_a_miss() {
        let Some((sa, sb)) = two_shared().await else { return };
        let (a, b) = (cache_with(sa.clone()), cache_with(sb));
        cache_put(&a, "p", small_response(), MINUTE).await;
        let key = entry_keys(&sa).await.remove(0);
        sa.run(|mut c| async move { redis::cmd("SET").arg(key).arg("not json").arg("PX").arg(60_000).query_async::<()>(&mut c).await })
            .await
            .unwrap();
        assert!(cache_get(&b, "p").await.is_none());
        assert!(cache_get(&a, "p").await.is_some(), "A's own copy is fine");
    }

    #[tokio::test]
    async fn invalidate_removes_the_answer_from_both_tiers() {
        let Some((sa, sb)) = two_shared().await else { return };
        let (a, b) = (cache_with(sa.clone()), cache_with(sb));
        cache_put(&a, "p", small_response(), MINUTE).await;
        cache_invalidate(&a, "p").await;
        assert!(cache_get(&a, "p").await.is_none());
        assert!(cache_get(&b, "p").await.is_none(), "gone from Redis too");
        assert!(entry_keys(&sa).await.is_empty());
    }

    #[test]
    fn images_past_the_cap_make_an_answer_too_big_without_serializing_it() {
        assert!(!too_big_to_share(&small_response()));
        assert!(!too_big_to_share(&response_with_b64(MAX_SHARED_ENTRY_BYTES)), "the exact check is `put`'s, with the JSON counted");
        assert!(too_big_to_share(&response_with_b64(MAX_SHARED_ENTRY_BYTES + 1)));
        let mut two = response_with_b64(MAX_SHARED_ENTRY_BYTES / 2 + 1);
        two.data.push(two.data[0].clone());
        assert!(too_big_to_share(&two), "the images add up");
    }

    /// What the log lines written while the guard lives said.
    #[derive(Clone, Default)]
    struct Logs(Arc<std::sync::Mutex<Vec<u8>>>);

    impl Logs {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl std::io::Write for Logs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
        type Writer = Logs;
        fn make_writer(&'a self) -> Logs {
            self.clone()
        }
    }

    #[test]
    fn the_unreadable_entry_warning_never_quotes_the_entry() {
        // A string where a number belongs: serde's own text would quote it.
        let secret = "https://img.example/a.png?signature=TOP-SECRET-SIGNATURE";
        let value = json!({ "response": { "created": secret } }).to_string();
        let Err(error) = serde_json::from_slice::<Entry>(value.as_bytes()) else { panic!("a type mismatch was expected") };
        assert!(error.to_string().contains(secret), "this value is one serde would echo: {error}");

        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt().with_writer(logs.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
        let guard = tracing::subscriber::set_default(subscriber);
        warn_unreadable(&Throttle::new(), &error);
        drop(guard);

        let text = logs.text();
        assert!(text.contains("a shared cache entry could not be read"), "the warning was captured: {text:?}");
        assert!(!text.contains("SIGNATURE") && !text.contains("img.example"), "never the cached text: {text}");
        assert!(text.contains("Data") && text.contains("line=1") && text.contains("column="), "category, line and column: {text}");
    }

    #[tokio::test]
    async fn an_unreadable_entry_reads_as_a_miss_not_an_error() {
        let Some((sa, sb)) = two_shared().await else { return };
        let key = sa.key(&["cache", "e", "bad"]);
        sa.run(|mut c| async move { redis::cmd("SET").arg(key).arg("{\"response\": 5}").arg("PX").arg(60_000).query_async::<()>(&mut c).await })
            .await
            .unwrap();
        assert_eq!(get(&sb, "bad").await.unwrap().map(|(r, _)| r.id), None, "a miss; it is not Redis failing");
    }

    #[tokio::test]
    async fn storing_an_answer_does_not_wait_for_a_slow_redis() {
        // A Redis that accepts connections and never answers.
        let proxy = CutProxy::start("127.0.0.1:1".parse().unwrap()).await;
        proxy.hang();
        let shared = SharedRedis::connect(&RedisSettings::from_url(&proxy.url()), &test_prefix()).await.unwrap();
        let cache = cache_with(shared);
        let started = std::time::Instant::now();
        cache.put_image(None, "m", &base("slow"), &extras(), &MaterializedRequest::default(), &small_response(), MINUTE).await;
        assert!(started.elapsed() < Duration::from_millis(250), "the response waited {:?} for Redis", started.elapsed());
        assert!(cache_get(&cache, "slow").await.is_some(), "kept locally at once");
        cache.flush_shared_writes().await;
    }

    /// A lookup that read an answer from Redis and then lost a race with a
    /// clear must not keep it: the clear would otherwise leave a stale answer
    /// served for its whole TTL. These two tests stop the lookup between
    /// Redis's reply and keeping the answer, and clear in that gap.
    #[tokio::test]
    async fn a_lookup_in_flight_when_another_replicas_clear_is_heard_does_not_keep_the_answer() {
        let Some((sa, sb)) = two_shared().await else { return };
        let (a, b) = (cache_with(sa), Arc::new(cache_with(sb)));
        cache_put(&a, "p", small_response(), MINUTE).await;

        let pause = b.pause_next_shared_lookup();
        let lookup = tokio::spawn({
            let b = b.clone();
            async move { cache_get(&b, "p").await }
        });
        pause.reached.notified().await;
        a.clear().await.unwrap();
        b.sync_epoch().await;
        pause.resume.notify_one();

        assert!(lookup.await.unwrap().is_some(), "the lookup answered what it had read");
        assert_eq!(b.entry_count().await, 0, "but B did not keep it past the clear");
        assert!(cache_get(&b, "p").await.is_none());
    }

    #[tokio::test]
    async fn a_lookup_in_flight_when_this_replica_clears_does_not_keep_the_answer() {
        let Some((sa, sb)) = two_shared().await else { return };
        let (a, b) = (Arc::new(cache_with(sa)), cache_with(sb));
        cache_put(&b, "p", small_response(), MINUTE).await;

        let pause = a.pause_next_shared_lookup();
        let lookup = tokio::spawn({
            let a = a.clone();
            async move { cache_get(&a, "p").await }
        });
        pause.reached.notified().await;
        a.clear().await.unwrap();
        pause.resume.notify_one();

        assert!(lookup.await.unwrap().is_some(), "the lookup answered what it had read");
        assert_eq!(a.entry_count().await, 0, "but A did not keep it after clearing");
        assert!(cache_get(&a, "p").await.is_none());
    }

    #[tokio::test]
    async fn a_dead_redis_still_clears_the_local_tier() {
        let c = cache_with(dead_redis(false).await);
        cache_put(&c, "x", small_response(), MINUTE).await;
        assert!(matches!(c.clear().await, Err(SharedError::Redis(_))), "the shared tier was not cleared, and the clear says so");
        assert!(cache_get(&c, "x").await.is_none());
        cache_put(&c, "y", small_response(), MINUTE).await;
        c.sync_epoch().await;
        assert!(cache_get(&c, "y").await.is_some(), "an epoch that cannot be read changes nothing");
    }
}
