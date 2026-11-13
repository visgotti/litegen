//! A per-address limit on the two OAuth endpoints anyone may call without
//! credentials that store or fetch something: `/oauth/register` (a client)
//! and `/oauth/authorize` (a pending request, and perhaps a fetch of the
//! client's metadata document). Each address may make
//! `mcp.oauth.rate_limit_per_minute` such requests at once, and gets them
//! back over a minute. Tokens, revocation and discovery are not limited: a
//! refresh never waits on a busy network.
//!
//! With Redis the allowance is shared: every replica draws from the same
//! bucket per address. Without it, or while Redis cannot be reached, each
//! replica counts for itself; with fail-closed on, a request that cannot be
//! counted against Redis is refused with 503 instead.
//!
//! A request counts against its connection's peer, unless the peer is a
//! proxy on this machine or a private network (loopback, RFC 1918, unique or
//! link local): then against the last address in `X-Forwarded-For`, the one
//! that proxy saw. An IPv6 address counts by its /64, which one subscriber
//! usually holds whole.

use std::collections::HashMap;
use std::net::{IpAddr, Ipv6Addr, SocketAddr};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use axum::extract::{ConnectInfo, Request, State};
use axum::http::{header, HeaderValue, Method, StatusCode};
use axum::middleware::Next;
use axum::response::{IntoResponse, Response};
use axum::Json;
use serde_json::json;

use super::OAuthShared;
use crate::api::middleware::rate_limit::{retry_after_secs, unverifiable_response, LimitRefusal};
use crate::shared::{rate_limit as shared_bucket, SharedError, SharedRedis, Throttle};

/// When this many addresses are tracked, those whose allowance is whole
/// again are forgotten.
const MAX_TRACKED: usize = 10_000;

struct Bucket {
    left: f64,
    at: Instant,
}

/// What each address has left of its allowance.
///
/// With Redis ([`AddressLimiter::with_shared`]) the allowances live there and
/// every replica draws from the same ones. Without it, or while Redis cannot
/// be reached and fail-closed is off, they are this process's own, in a map
/// here. The map is never cleared when Redis answers again: a Redis that
/// flaps must not hand every address a full allowance at each failure.
#[derive(Default)]
pub struct AddressLimiter {
    local: Mutex<HashMap<IpAddr, Bucket>>,
    shared: Option<Arc<SharedRedis>>,
    /// Lets the log say that Redis is out once a minute, not once a request.
    outage_warning: Throttle,
}

impl AddressLimiter {
    /// Allowances shared through `shared`, falling back to this process's own
    /// while Redis cannot be reached (or refusing, if `shared.fail_closed()`).
    pub fn with_shared(shared: Arc<SharedRedis>) -> Self {
        Self { shared: Some(shared), ..Self::default() }
    }

    /// Spend one of `address`'s `per_minute` requests, or say why not:
    /// [`LimitRefusal::Exceeded`] with the seconds until one is back, or
    /// [`LimitRefusal::Unverifiable`] when Redis is out and fail-closed is on.
    /// `address` is already the one a request counts against (an IPv6 address
    /// by its /64, see [`client_address`]). `now` serves this process's own
    /// map only.
    pub async fn try_take(&self, address: IpAddr, per_minute: u32, now: Instant) -> Result<(), LimitRefusal> {
        if let Some(shared) = &self.shared {
            // Every replica names the address's bucket alike, so they share it.
            match shared_bucket::take(shared, &format!("signin:{address}"), per_minute).await {
                Ok((true, _)) => return Ok(()),
                Ok((false, tokens)) => return Err(LimitRefusal::Exceeded { retry_after_secs: retry_after_secs(tokens, per_minute) }),
                Err(e) if shared.fail_closed() => {
                    self.warn_outage(&e, "sign-in requests are refused with 503 until it works again");
                    return Err(LimitRefusal::Unverifiable);
                }
                Err(e) => self.warn_outage(&e, "limits are per replica until it works again"),
            }
        }
        self.take_local(address, per_minute, now)
    }

    /// Spend one of `address`'s requests from this process's own map.
    fn take_local(&self, address: IpAddr, per_minute: u32, now: Instant) -> Result<(), LimitRefusal> {
        let whole = f64::from(per_minute);
        let per_second = whole / 60.0;
        let mut buckets = self.local.lock().unwrap_or_else(|e| e.into_inner());
        if buckets.len() >= MAX_TRACKED {
            buckets.retain(|_, b| now.saturating_duration_since(b.at) < Duration::from_secs(60));
        }
        let bucket = buckets.entry(address).or_insert(Bucket { left: whole, at: now });
        bucket.left = (bucket.left + now.saturating_duration_since(bucket.at).as_secs_f64() * per_second).min(whole);
        bucket.at = bucket.at.max(now);
        if bucket.left >= 1.0 {
            bucket.left -= 1.0;
            Ok(())
        } else {
            // Here `left < 1`, so the wait is `ceil((1 - left) / refill_rate)`:
            // positive, and a whole number of seconds, so at least 1. That is
            // why there is no `.max(1)`: `by_address` sends it as `Retry-After`,
            // and a 0 there would ask the client to retry at once. The shared
            // path refuses only with `tokens < 1` as well.
            Err(LimitRefusal::Exceeded { retry_after_secs: retry_after_secs(bucket.left, per_minute) })
        }
    }

    /// Say that Redis sign-in limiting failed, at most once a minute. The
    /// cause may be an unreachable Redis or an error reply from one that
    /// answers. `consequence` is what the operator should expect meanwhile.
    /// `error` is Redis's or the network's own text, never the URL.
    fn warn_outage(&self, error: &SharedError, consequence: &str) {
        if self.outage_warning.due(Instant::now()) {
            tracing::warn!(%error, "Redis sign-in limiting failed; {consequence}");
        }
    }

    /// How many addresses this process's own map tracks.
    #[cfg(test)]
    pub(super) fn tracked_addresses(&self) -> usize {
        self.local.lock().unwrap_or_else(|e| e.into_inner()).len()
    }
}

/// The address a request counts against, from its connection's `peer` and
/// its `X-Forwarded-For`; None with no connection (an in-process call).
pub fn client_address(peer: Option<SocketAddr>, forwarded_for: Option<&str>) -> Option<IpAddr> {
    let peer = canonical(peer?.ip());
    let address = match forwarded_for {
        Some(header) if is_local(peer) => header.rsplit(',').next().and_then(|last| parse_forwarded(last.trim())).map(canonical).unwrap_or(peer),
        _ => peer,
    };
    Some(match address {
        IpAddr::V6(v6) => IpAddr::V6(Ipv6Addr::from(u128::from(v6) & !((1u128 << 64) - 1))),
        v4 => v4,
    })
}

/// An `X-Forwarded-For` entry: an address, perhaps with a port.
fn parse_forwarded(entry: &str) -> Option<IpAddr> {
    entry.parse::<IpAddr>().ok().or_else(|| entry.parse::<SocketAddr>().ok().map(|s| s.ip()))
}

/// IPv4-mapped IPv6 as the IPv4 address it is.
fn canonical(ip: IpAddr) -> IpAddr {
    match ip {
        IpAddr::V6(v6) => v6.to_ipv4_mapped().map(IpAddr::V4).unwrap_or(IpAddr::V6(v6)),
        v4 => v4,
    }
}

/// Whether `ip` is on this machine or a private network: a proxy LiteGen
/// sits behind.
fn is_local(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => v4.is_loopback() || v4.is_private() || v4.is_link_local(),
        IpAddr::V6(v6) => v6.is_loopback() || (v6.segments()[0] & 0xfe00) == 0xfc00 || (v6.segments()[0] & 0xffc0) == 0xfe80,
    }
}

/// Refuse a request from an address that spent its allowance: 429 with
/// `Retry-After`, as a page for the browser that opened `/oauth/authorize`
/// and as an OAuth error for a client registering. With Redis and fail-closed
/// on, while Redis can't be reached, the answer is a 503 with
/// `Retry-After: 1` instead, since the allowance can't be checked: a page for
/// the browser, and the fixed `rate_limit_unverifiable` error otherwise.
pub async fn by_address(State(shared): State<Arc<OAuthShared>>, request: Request, next: Next) -> Response {
    let per_minute = shared.state.router.config.mcp.oauth.rate_limit_per_minute;
    if per_minute == 0 {
        return next.run(request).await;
    }
    let peer = request.extensions().get::<ConnectInfo<SocketAddr>>().map(|ConnectInfo(peer)| *peer);
    let forwarded: Vec<&str> = request.headers().get_all("x-forwarded-for").iter().filter_map(|v| v.to_str().ok()).collect();
    let forwarded = (!forwarded.is_empty()).then(|| forwarded.join(","));
    let Some(address) = client_address(peer, forwarded.as_deref()) else {
        return next.run(request).await;
    };
    let wait = match shared.limiter.try_take(address, per_minute, Instant::now()).await {
        Ok(()) => return next.run(request).await,
        Err(LimitRefusal::Unverifiable) if request.method() == Method::GET => {
            let mut page = super::authorize::unavailable_page();
            page.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(1u64));
            return page;
        }
        Err(LimitRefusal::Unverifiable) => return unverifiable_response(),
        Err(LimitRefusal::Exceeded { retry_after_secs: wait }) => wait,
    };
    tracing::info!(%address, path = request.uri().path(), "MCP OAuth request over the per-address limit");
    let mut response = if request.method() == Method::GET {
        super::authorize::busy_page(wait)
    } else {
        let description = format!("too many sign-in requests from this network; try again in {wait} seconds");
        (StatusCode::TOO_MANY_REQUESTS, [(header::CACHE_CONTROL, "no-store")], Json(json!({ "error": "temporarily_unavailable", "error_description": description })))
            .into_response()
    };
    response.headers_mut().insert(header::RETRY_AFTER, HeaderValue::from(wait));
    response
}
