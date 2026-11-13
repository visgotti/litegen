//! The settings in force for the cache, retries, the vendor timeout and the
//! circuit breaker (`docs/gateway-settings.md`): each the gateway setting
//! stored in the database, else `litegen.yaml`'s.

use std::time::Duration;

use crate::config::AppConfig;
use crate::management::types::{EffectiveBreaker, EffectiveCache, EffectiveRetries, GatewaySettings};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CacheSettings {
    pub enabled: bool,
    pub ttl: Duration,
    pub max_items: u64,
    /// About how many bytes the local tier may hold (P7).
    pub max_bytes: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RetrySettings {
    /// Tries after the first.
    pub max_retries: u32,
    /// The longest first wait.
    pub backoff: Duration,
    /// The longest any wait may be; never shorter than `backoff`.
    pub max_backoff: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BreakerSettings {
    pub threshold: u32,
    pub open_for: Duration,
}

/// Everything the cache, the retry loops, vendor calls and the circuit
/// breaker read at each request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EffectiveSettings {
    pub cache: CacheSettings,
    pub retries: RetrySettings,
    pub vendor_timeout: Duration,
    pub breaker: BreakerSettings,
}

/// Each field: the gateway setting, else the config's.
pub fn effective(config: &AppConfig, gateway: &GatewaySettings) -> EffectiveSettings {
    let backoff = Duration::from_millis(gateway.retry_backoff_ms.unwrap_or(config.retries.backoff_ms));
    let max_backoff = Duration::from_millis(gateway.retry_max_backoff_ms.unwrap_or(config.retries.max_backoff_ms));
    EffectiveSettings {
        cache: CacheSettings {
            enabled: gateway.cache_enabled.unwrap_or(config.cache.enabled),
            ttl: Duration::from_secs(gateway.cache_ttl_seconds.unwrap_or(config.cache.default_ttl_seconds)),
            max_items: gateway.cache_max_items.unwrap_or(config.cache.max_items),
            max_bytes: gateway.cache_max_bytes.unwrap_or(config.cache.max_bytes),
        },
        retries: RetrySettings {
            max_retries: gateway.retry_max_retries.unwrap_or(config.retries.max_retries),
            backoff,
            // Each layer is valid alone; together they may cross.
            max_backoff: max_backoff.max(backoff),
        },
        vendor_timeout: gateway.vendor_timeout_seconds.map(Duration::from_secs).unwrap_or_else(|| config.server.vendor_timeout()),
        breaker: BreakerSettings {
            threshold: gateway.circuit_breaker_threshold.unwrap_or(config.circuit_breaker.threshold),
            open_for: Duration::from_secs(gateway.circuit_breaker_open_for_seconds.unwrap_or(config.circuit_breaker.open_for_seconds)),
        },
    }
}

impl From<CacheSettings> for EffectiveCache {
    fn from(c: CacheSettings) -> Self {
        Self { enabled: c.enabled, ttl_seconds: c.ttl.as_secs(), max_items: c.max_items, max_bytes: c.max_bytes }
    }
}

impl From<RetrySettings> for EffectiveRetries {
    fn from(r: RetrySettings) -> Self {
        Self { max_retries: r.max_retries, backoff_ms: r.backoff.as_millis() as u64, max_backoff_ms: r.max_backoff.as_millis() as u64 }
    }
}

impl From<BreakerSettings> for EffectiveBreaker {
    fn from(b: BreakerSettings) -> Self {
        Self { threshold: b.threshold, open_for_seconds: b.open_for.as_secs() }
    }
}


#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::AppConfig;
    use crate::management::types::GatewaySettings;
    use std::time::Duration;

    #[test]
    fn effective_settings_layer_the_gateway_over_the_yaml() {
        let mut config = AppConfig::default();
        config.cache.enabled = true;
        config.cache.default_ttl_seconds = 600;
        config.retries.backoff_ms = 250;
        config.server.vendor_timeout_secs = Some(60);
        config.circuit_breaker.threshold = 3;

        let yaml_only = effective(&config, &GatewaySettings::default());
        assert_eq!(yaml_only.cache, CacheSettings { enabled: true, ttl: Duration::from_secs(600), max_items: 10_000, max_bytes: 268_435_456 });
        assert_eq!(yaml_only.retries, RetrySettings { max_retries: 2, backoff: Duration::from_millis(250), max_backoff: Duration::from_secs(10) });
        assert_eq!(yaml_only.vendor_timeout, Duration::from_secs(60));
        assert_eq!(yaml_only.breaker, BreakerSettings { threshold: 3, open_for: Duration::from_secs(30) });

        let gateway = GatewaySettings {
            cache_enabled: Some(false),
            cache_max_items: Some(50),
            cache_max_bytes: Some(2_097_152),
            retry_max_retries: Some(4),
            retry_max_backoff_ms: Some(2000),
            vendor_timeout_seconds: Some(30),
            circuit_breaker_open_for_seconds: Some(5),
            ..Default::default()
        };
        let both = effective(&config, &gateway);
        assert_eq!(both.cache, CacheSettings { enabled: false, ttl: Duration::from_secs(600), max_items: 50, max_bytes: 2_097_152 });
        assert_eq!(both.retries, RetrySettings { max_retries: 4, backoff: Duration::from_millis(250), max_backoff: Duration::from_secs(2) });
        assert_eq!(both.vendor_timeout, Duration::from_secs(30));
        assert_eq!(both.breaker, BreakerSettings { threshold: 3, open_for: Duration::from_secs(5) });
    }

    #[test]
    fn the_longest_backoff_is_never_shorter_than_the_first() {
        // Each layer valid on its own can still cross: the gateway's first
        // backoff above the YAML's ceiling.
        let config = AppConfig::default();
        let gateway = GatewaySettings { retry_backoff_ms: Some(20_000), ..Default::default() };
        let s = effective(&config, &gateway).retries;
        assert_eq!((s.backoff, s.max_backoff), (Duration::from_secs(20), Duration::from_secs(20)));
    }
}
