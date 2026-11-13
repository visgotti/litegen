//! The token bucket that all replicas share: one Redis script, one atomic
//! step per request (`docs/scaling.md`).
//!
//! The math is the local bucket's (`api::middleware::rate_limit`): capacity
//! `rpm`, refilled at `rpm / 60` a second, starting full. Each bucket is a hash
//! under `key(["rl", bucket])` with the fields `tokens` and `ts_ms`, and it
//! expires 120 s after its last write. A bucket refills completely in 60 s, so
//! an expired bucket and a full one are the same thing.
//!
//! Two rules keep the replicas honest:
//!
//! - **One step.** Reading the bucket, refilling it and taking a token happen
//!   in one `EVAL`, which Redis runs without interleaving another command. Two
//!   replicas taking at the same instant cannot both see the last token.
//! - **One clock.** The time is `redis.call('TIME')`, never a timestamp a
//!   replica sends. Replicas' clocks differ, and a replica that is a second
//!   ahead would refill the bucket early on every take.
//!
//! The script reads the clock and then writes. Redis 5 and later (and Valkey)
//! allow that, because they replicate a script's effects, not its text.
//!
//! The script runs through [`redis::Script`]: it is sent by hash, and loaded
//! again when Redis answers `NOSCRIPT` (after a restart, a failover or
//! `SCRIPT FLUSH`).

use std::sync::LazyLock;

use super::{SharedError, SharedRedis};

/// Takes one token from the bucket in `KEYS[1]`, or only looks.
///
/// - `ARGV[1]`: the capacity, which is the requests per minute.
/// - `ARGV[2]`: `1` to take a token, `0` to look without changing anything.
///
/// Returns `{allowed, tokens}`: `allowed` is 1 or 0, and `tokens` is the
/// bucket after the call, as a string. Redis turns a Lua number into an
/// integer when it replies, which would lose the fraction.
pub const SCRIPT_SOURCE: &str = r#"
local capacity = tonumber(ARGV[1])
local t = redis.call('TIME')
local now = tonumber(t[1]) * 1000 + math.floor(tonumber(t[2]) / 1000)
local state = redis.call('HMGET', KEYS[1], 'tokens', 'ts_ms')
local tokens = tonumber(state[1])
local ts = tonumber(state[2])
if tokens == nil or ts == nil then
  tokens = capacity
  ts = now
end
local elapsed = now - ts
if elapsed < 0 then
  elapsed = 0
end
tokens = math.min(capacity, tokens + elapsed * capacity / 60000)
local allowed = 0
if ARGV[2] == '1' then
  if tokens >= 1 then
    tokens = tokens - 1
    allowed = 1
  end
  redis.call('HSET', KEYS[1], 'tokens', string.format('%.17g', tokens), 'ts_ms', string.format('%.0f', now))
  redis.call('PEXPIRE', KEYS[1], 120000)
end
return {allowed, string.format('%.17g', tokens)}
"#;

/// Built once, so its SHA-1 is computed once.
static SCRIPT: LazyLock<redis::Script> = LazyLock::new(|| redis::Script::new(SCRIPT_SOURCE));

/// Take one token from `bucket` at `rpm` requests a minute. Returns whether
/// there was one, and how many tokens the bucket holds afterwards (the
/// fraction included). A new bucket starts full.
pub async fn take(shared: &SharedRedis, bucket: &str, rpm: u32) -> Result<(bool, f64), SharedError> {
    call(shared, bucket, rpm, true).await
}

/// How many tokens `bucket` holds now at `rpm`, with nothing taken and
/// nothing written. A bucket nobody has taken from is full.
pub async fn peek(shared: &SharedRedis, bucket: &str, rpm: u32) -> Result<f64, SharedError> {
    call(shared, bucket, rpm, false).await.map(|(_, tokens)| tokens)
}

async fn call(shared: &SharedRedis, bucket: &str, rpm: u32, take: bool) -> Result<(bool, f64), SharedError> {
    let key = shared.key(&["rl", bucket]);
    let (allowed, tokens): (i64, String) = shared
        .run(|mut connection| async move {
            let mut invocation = SCRIPT.prepare_invoke();
            invocation.key(key.as_str()).arg(rpm).arg(if take { 1 } else { 0 });
            invocation.invoke_async(&mut connection).await
        })
        .await?;
    let tokens = tokens
        .parse::<f64>()
        .map_err(|_| SharedError::Redis(format!("the rate-limit script returned {tokens:?}, not a number")))?;
    Ok((allowed == 1, tokens))
}
