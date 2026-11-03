//! A per-address limit on the two OAuth endpoints anyone may call without
//! credentials that store or fetch something: `/oauth/register` (a client)
//! and `/oauth/authorize` (a pending request, and perhaps a fetch of the
//! client's metadata document). Each address may make
//! `mcp.oauth.rate_limit_per_minute` such requests at once, and gets them
//! back over a minute. Tokens, revocation and discovery are not limited: a
//! refresh never waits on a busy network.
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

/// When this many addresses are tracked, those whose allowance is whole
/// again are forgotten.
const MAX_TRACKED: usize = 10_000;

struct Bucket {
    left: f64,
    at: Instant,
}

/// What each address has left of its allowance.
#[derive(Default)]
pub struct AddressLimiter(Mutex<HashMap<IpAddr, Bucket>>);

impl AddressLimiter {
    /// Spend one of `address`'s `per_minute` requests at `now`, or say in
    /// how many seconds one is back.
    pub fn try_take(&self, address: IpAddr, per_minute: u32, now: Instant) -> Result<(), u64> {
        let whole = f64::from(per_minute);
        let per_second = whole / 60.0;
        let mut buckets = self.0.lock().unwrap_or_else(|e| e.into_inner());
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
            Err(((1.0 - bucket.left) / per_second).ceil().max(1.0) as u64)
        }
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
/// and as an OAuth error for a client registering.
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
    let Err(wait) = shared.limiter.try_take(address, per_minute, Instant::now()) else {
        return next.run(request).await;
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
