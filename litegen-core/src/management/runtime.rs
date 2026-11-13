//! Runtime configuration: loading what the management API stored at boot, and
//! the helpers that apply a change to the live gateway.
//!
//! - Gateway settings and model routes live on the router: set whole at boot
//!   and by a refresh ([`ProxyRouter::set_overrides`](crate::proxy::router::ProxyRouter::set_overrides)),
//!   changed one field at a time by a save
//!   ([`ProxyRouter::update_overrides`](crate::proxy::router::ProxyRouter::update_overrides)).
//! - Runtime models and disabled ids live on the capability registry's runtime
//!   layer, so the validator and `/v1/models` see them immediately.
//! - Provider configs rebuild the provider instance in the provider registry.
//!   A job already in flight keeps the instance it submitted with.


use std::collections::{HashMap, HashSet};
use std::sync::{Arc, LazyLock, Mutex};
use std::time::{Duration, Instant};

use chrono::{DateTime, Utc};
use tokio::sync::Notify;

use super::store::ManagementStore;
use super::types::*;
use crate::api::middleware::AppState;
use crate::capabilities::schema::{MediaType, ModelSchema};
use crate::config::{CacheRouteConfig, DeploymentConfig, ModelRouteConfig, ProviderEnvConfig};
use crate::proxy::registry::{IMAGE_PROVIDERS, MODEL3D_PROVIDERS, VIDEO_PROVIDERS};
use crate::shared::events::SharedEvent;
use crate::shared::Throttle;

/// `gateway_settings` key holding the [`GatewaySettings`] JSON.
pub const SETTINGS_KEY: &str = "gateway";
/// `gateway_settings` key holding the runtime model routes JSON.
pub const ROUTES_KEY: &str = "model_routes";

/// Providers whose adapter maps model ids through a fixed endpoint table: a
/// model id the table does not know would silently call a different model, so
/// runtime models cannot target them.
const TABLE_ROUTED_PROVIDERS: &[&str] = &["fal"];

// ─── Gateway settings ────────────────────────────────────────────────────────

pub async fn load_gateway_settings(store: &dyn ManagementStore) -> Result<GatewaySettings, sqlx::Error> {
    Ok(store
        .get_setting(SETTINGS_KEY)
        .await?
        .and_then(|s| serde_json::from_str(&s).ok())
        .unwrap_or_default())
}

pub async fn load_routes(store: &dyn ManagementStore) -> Result<Option<Vec<RouteSpec>>, sqlx::Error> {
    Ok(store.get_setting(ROUTES_KEY).await?.and_then(|s| serde_json::from_str(&s).ok()))
}

pub fn validate_gateway_settings(s: &GatewaySettings) -> Result<(), String> {
    if let Some(m) = s.cost_markup_percent {
        if !m.is_finite() || !(0.0..=1000.0).contains(&m) {
            return Err("cost_markup_percent must be between 0 and 1000".into());
        }
    }
    if let Some(url) = &s.alert_webhook_url {
        let ok = reqwest::Url::parse(url).map(|u| matches!(u.scheme(), "http" | "https")).unwrap_or(false);
        if !ok {
            return Err("alert_webhook_url must be an http(s) URL".into());
        }
    }
    if let Some(b) = s.default_end_user_max_budget_usd {
        if !b.is_finite() || b < 0.0 {
            return Err("default_end_user_max_budget_usd must be a non-negative number".into());
        }
    }
    if let Some(d) = &s.default_end_user_budget_duration {
        super::budget::parse_duration(d)?;
    }
    let range = |v: Option<u64>, field: &str, lo: u64, hi: u64| match v {
        Some(v) if !(lo..=hi).contains(&v) => Err(format!("{field} must be between {lo} and {hi}")),
        _ => Ok(()),
    };
    range(s.cache_ttl_seconds, "cache_ttl_seconds", 1, 604_800)?;
    range(s.cache_max_items, "cache_max_items", 1, 1_000_000)?;
    range(s.cache_max_bytes, "cache_max_bytes", 1_048_576, 68_719_476_736)?;
    range(s.retry_max_retries.map(u64::from), "retry_max_retries", 0, 5)?;
    range(s.retry_backoff_ms, "retry_backoff_ms", 0, 60_000)?;
    range(s.retry_max_backoff_ms, "retry_max_backoff_ms", 0, 60_000)?;
    if let (Some(first), Some(longest)) = (s.retry_backoff_ms, s.retry_max_backoff_ms) {
        if longest < first {
            return Err("retry_max_backoff_ms must be at least retry_backoff_ms".into());
        }
    }
    range(s.vendor_timeout_seconds, "vendor_timeout_seconds", 5, 3600)?;
    range(s.circuit_breaker_threshold.map(u64::from), "circuit_breaker_threshold", 1, 1000)?;
    range(s.circuit_breaker_open_for_seconds, "circuit_breaker_open_for_seconds", 1, 3600)?;
    if let Some(t) = s.storage_access_signed_url_ttl_seconds {
        if !crate::access::ttl_in_range(i64::from(t)) {
            return Err(format!(
                "storage_access_signed_url_ttl_seconds must be between {} and {} seconds",
                crate::access::MIN_SIGNED_URL_TTL_SECONDS,
                crate::access::MAX_SIGNED_URL_TTL_SECONDS,
            ));
        }
    }
    range(s.storage_access_retention_days.map(u64::from), "storage_access_retention_days", 0, u64::from(crate::access::MAX_RETENTION_DAYS))?;
    Ok(())
}

/// POST a budget alert to the configured alert webhook, off the request path.
pub async fn send_alert(state: &AppState, payload: serde_json::Value) {
    let Some(url) = state.router.overrides().gateway.alert_webhook_url else { return };
    tokio::spawn(async move {
        static CLIENT: once_cell::sync::Lazy<reqwest::Client> = once_cell::sync::Lazy::new(|| {
            reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(5))
                .build()
                .unwrap_or_default()
        });
        let event = payload.get("type").and_then(|v| v.as_str()).unwrap_or("budget.alert").to_string();
        if let Err(e) = CLIENT.post(&url).header("x-litegen-event", event).json(&payload).send().await {
            tracing::warn!(error = %e, "budget alert webhook failed");
        }
    });
}

/// Give a first-seen end user the gateway's default budget, if one is set.
pub async fn ensure_default_end_user_limits(
    state: &AppState,
    store: &dyn ManagementStore,
    org_id: &str,
    end_user_id: &str,
) -> Result<(), sqlx::Error> {
    let gateway = state.router.overrides().gateway;
    let Some(max) = gateway.default_end_user_max_budget_usd else { return Ok(()) };
    let entity_id = end_user_entity_id(org_id, end_user_id);
    if store.get_limits(EntityType::EndUser, &entity_id).await?.is_some() {
        return Ok(());
    }
    let now = Utc::now();
    let duration = gateway.default_end_user_budget_duration.clone();
    let reset = super::budget::initial_reset_at(duration.as_deref(), now).unwrap_or(None);
    store
        .upsert_limits(&EntityLimits {
            entity_type: EntityType::EndUser,
            entity_id,
            org_id: Some(org_id.to_string()),
            max_budget_usd: Some(max),
            soft_budget_usd: None,
            budget_duration: if reset.is_some() { duration } else { None },
            spend_usd: 0.0,
            budget_reset_at: reset,
            soft_budget_alerted_at: None,
            allowed_models: None,
            max_parallel_requests: None,
            rpm_limit: None,
            metadata: None,
            created_at: now,
            updated_at: now,
        })
        .await
}

// ─── Routes ──────────────────────────────────────────────────────────────────

const STRATEGIES: &[&str] = &["fallback", "weighted_round_robin", "lowest_cost", "lowest_latency"];

pub fn known_provider(name: &str) -> bool {
    IMAGE_PROVIDERS.contains(&name) || VIDEO_PROVIDERS.contains(&name) || MODEL3D_PROVIDERS.contains(&name)
}

pub fn validate_routes(routes: &[RouteSpec]) -> Result<(), String> {
    for (i, r) in routes.iter().enumerate() {
        if r.model.trim().is_empty() {
            return Err(format!("routes[{i}].model must not be empty"));
        }
        if r.deployments.is_empty() {
            return Err(format!("routes[{i}] ('{}') needs at least one deployment", r.model));
        }
        for d in &r.deployments {
            if !known_provider(&d.provider) {
                return Err(format!("routes[{i}] ('{}'): unknown provider '{}'", r.model, d.provider));
            }
            if d.weight == 0 {
                return Err(format!("routes[{i}] ('{}'): weight must be at least 1", r.model));
            }
        }
        if let Some(s) = r.strategy.as_deref() {
            if !STRATEGIES.contains(&s) {
                return Err(format!("routes[{i}] ('{}'): strategy must be one of {}", r.model, STRATEGIES.join(", ")));
            }
        }
    }
    Ok(())
}

pub fn route_spec_to_config(r: &RouteSpec) -> ModelRouteConfig {
    ModelRouteConfig {
        model: r.model.clone(),
        deployments: r
            .deployments
            .iter()
            .map(|d| DeploymentConfig {
                provider: d.provider.clone(),
                weight: d.weight,
                max_retries: d.max_retries,
                timeout_seconds: d.timeout_seconds,
                rpm_limit: d.rpm_limit,
            })
            .collect(),
        strategy: r.strategy.clone(),
        cache: r.cache_enabled.map(|enabled| CacheRouteConfig {
            enabled,
            ttl_seconds: r.cache_ttl_seconds.unwrap_or(3600),
        }),
    }
}

pub fn route_config_to_spec(r: &ModelRouteConfig) -> RouteSpec {
    RouteSpec {
        model: r.model.clone(),
        deployments: r
            .deployments
            .iter()
            .map(|d| RouteDeployment {
                provider: d.provider.clone(),
                weight: d.weight,
                max_retries: d.max_retries,
                timeout_seconds: d.timeout_seconds,
                rpm_limit: d.rpm_limit,
            })
            .collect(),
        strategy: r.strategy.clone(),
        cache_enabled: r.cache.as_ref().map(|c| c.enabled),
        cache_ttl_seconds: r.cache.as_ref().map(|c| c.ttl_seconds),
    }
}

// ─── Runtime models ──────────────────────────────────────────────────────────

/// Check a model posted to the API with the catalog loader's own rules, plus
/// what only matters at runtime: the id must not shadow a catalog model and
/// the provider must have an adapter for the media type that can route an
/// arbitrary vendor model id (the part of the id after `<provider>/`).
pub fn validate_runtime_model(state: &AppState, m: &ModelSchema) -> Result<(), String> {
    crate::capabilities::loader::validate_model("runtime model", m).map_err(|e| match e {
        crate::capabilities::loader::LoadError::Validate { message, .. } => message,
        other => other.to_string(),
    })?;
    if state.registry.is_catalog(&m.id) {
        return Err(format!("'{}' is a catalog model (models/*.yaml) and cannot be redefined through the API", m.id));
    }
    let supported = match m.media_type {
        MediaType::Image => IMAGE_PROVIDERS,
        MediaType::Video => VIDEO_PROVIDERS,
        MediaType::Model3d => MODEL3D_PROVIDERS,
    };
    if !supported.contains(&m.provider.as_str()) {
        return Err(format!("provider '{}' has no {:?} adapter", m.provider, m.media_type).to_lowercase());
    }
    if TABLE_ROUTED_PROVIDERS.contains(&m.provider.as_str()) {
        return Err(format!(
            "provider '{}' maps model ids through a fixed endpoint table, so a runtime model could never reach the vendor model it names; add it to models/{}.yaml instead",
            m.provider, m.provider
        ));
    }
    Ok(())
}

// ─── Provider configs ────────────────────────────────────────────────────────

/// Decrypt a stored config's secrets.
pub fn decrypt_secrets(state: &AppState, stored: &StoredProviderConfig) -> Result<ProviderSecrets, String> {
    let (Some(ct), Some(nonce)) = (&stored.ciphertext, &stored.nonce) else { return Ok(ProviderSecrets::default()) };
    let key = state.secrets_key.as_ref().ok_or("secrets key not configured (LITEGEN__SECRETS_KEY)")?;
    let plain = crate::auth::secrets::decrypt(key, ct, nonce)?;
    serde_json::from_slice(&plain).map_err(|e| e.to_string())
}

pub fn parse_settings(stored: &StoredProviderConfig) -> ProviderSettings {
    serde_json::from_str(&stored.settings_json).unwrap_or_default()
}

fn blank_env_config() -> ProviderEnvConfig {
    serde_json::from_value(serde_json::json!({})).expect("every ProviderEnvConfig field has a default")
}

/// The env/YAML config for `name`, if the operator set one.
pub fn env_config(state: &AppState, name: &str) -> Option<ProviderEnvConfig> {
    state.router.config.providers.get(name).cloned()
}

/// Layer a runtime config over the env config: every field the runtime config
/// sets wins, the rest falls through.
pub fn effective_config(
    env: Option<&ProviderEnvConfig>,
    secrets: &ProviderSecrets,
    settings: &ProviderSettings,
) -> ProviderEnvConfig {
    let mut c = env.cloned().unwrap_or_else(blank_env_config);
    if let Some(v) = &secrets.api_key { c.api_key = Some(v.clone()); }
    if let Some(v) = &secrets.api_keys { c.api_keys = Some(v.clone()); }
    if let Some(v) = &secrets.key_id { c.key_id = Some(v.clone()); }
    if let Some(v) = &secrets.key_secret { c.key_secret = Some(v.clone()); }
    if let Some(v) = &settings.api_base { c.api_base = Some(v.clone()); }
    if let Some(v) = &settings.model_mapping { c.model_mapping = v.clone(); }
    if let Some(v) = &settings.extra_headers { c.extra_headers = v.clone(); }
    if let Some(v) = &settings.options { c.options = Some(v.clone()); }
    if let Some(v) = &settings.region { c.region = Some(v.clone()); }
    if let Some(v) = settings.enabled { c.enabled = v; }
    c
}

/// Rebuild provider `name` from its env config plus any stored runtime config,
/// registering or unregistering it. Returns whether it is now serving.
pub async fn apply_provider(state: &AppState, store: &dyn ManagementStore, name: &str) -> Result<bool, String> {
    let stored = store.get_provider_config(name).await.map_err(|e| e.to_string())?;
    let env = env_config(state, name);
    let (secrets, settings) = match &stored {
        Some(s) => (decrypt_secrets(state, s)?, parse_settings(s)),
        None => (ProviderSecrets::default(), ProviderSettings::default()),
    };
    if stored.is_none() && env.is_none() {
        state.router.registry.unregister_provider(name).await;
        return Ok(false);
    }
    let effective = effective_config(env.as_ref(), &secrets, &settings);
    let instance = crate::proxy::registry::build_instance_config(&effective);
    if !effective.enabled || (name != "mock" && !instance.credentials.any_present()) {
        state.router.registry.unregister_provider(name).await;
        return Ok(false);
    }
    Ok(state.router.registry.configure_provider(name, instance).await)
}

/// `…a1b2` for a secret, never more than its last four characters.
pub fn key_hint(secret: &str) -> Option<String> {
    let chars: Vec<char> = secret.chars().collect();
    if chars.len() < 8 {
        return Some("…".to_string());
    }
    Some(format!("…{}", chars[chars.len() - 4..].iter().collect::<String>()))
}

/// The masked admin view of provider `name`.
pub async fn provider_view(state: &AppState, store: &dyn ManagementStore, name: &str) -> Result<ProviderConfigView, String> {
    let stored = store.get_provider_config(name).await.map_err(|e| e.to_string())?;
    let env = env_config(state, name);
    let (secrets, settings) = match &stored {
        // A view must render even if the secret no longer decrypts.
        Some(s) => (decrypt_secrets(state, s).unwrap_or_default(), parse_settings(s)),
        None => (ProviderSecrets::default(), ProviderSettings::default()),
    };
    let effective = effective_config(env.as_ref(), &secrets, &settings);
    let mut modalities = Vec::new();
    if IMAGE_PROVIDERS.contains(&name) { modalities.push("image".to_string()); }
    if VIDEO_PROVIDERS.contains(&name) { modalities.push("video".to_string()); }
    if MODEL3D_PROVIDERS.contains(&name) { modalities.push("model3d".to_string()); }
    let active = state.router.registry.registered_config(name).await.is_some();
    let secret = effective
        .api_key
        .clone()
        .filter(|s| !s.is_empty())
        .or_else(|| effective.api_keys.as_deref().and_then(|k| k.split(',').next()).map(|k| k.split(':').next().unwrap_or(k).to_string()))
        .or_else(|| effective.key_id.clone());
    let mut header_names: Vec<String> = effective.extra_headers.keys().cloned().collect();
    header_names.sort();
    Ok(ProviderConfigView {
        name: name.to_string(),
        modalities,
        active,
        source: if stored.is_some() { "runtime" } else if env.is_some() { "env" } else { "none" }.to_string(),
        key_hint: stored.as_ref().and_then(|s| s.display_hint.clone()).or_else(|| secret.as_deref().and_then(key_hint)),
        has_secret: secret.is_some() || effective.key_secret.is_some(),
        api_base: effective.api_base.clone(),
        region: effective.region.clone(),
        model_mapping: effective.model_mapping.clone(),
        extra_header_names: header_names,
        options: effective.options.clone(),
        enabled: effective.enabled,
        updated_at: stored.map(|s| s.updated_at),
    })
}

/// Every adapter name litegen knows, sorted.
pub fn all_provider_names() -> Vec<&'static str> {
    let mut names: Vec<&'static str> = Vec::new();
    for n in IMAGE_PROVIDERS.iter().chain(VIDEO_PROVIDERS).chain(MODEL3D_PROVIDERS) {
        if !names.contains(n) {
            names.push(n);
        }
    }
    names.sort_unstable();
    names
}

// ─── Boot and refresh ────────────────────────────────────────────────────────

/// What this replica has applied of the stored runtime models and provider
/// configs: each id with the `updated_at` of the row it applied. Boot and
/// [`refresh`] write it, and nothing else does, so it never claims more than
/// they applied. A row that does not validate is recorded as seen too, though
/// none of it is served, so it is not parsed and warned about again until it
/// changes. A save made on this replica through the API is not in it until the
/// next refresh re-applies that row, which changes nothing.
///
/// That is also how a refresh that raced such a save heals. If it applied an
/// older row after the save's own apply, the record holds the older
/// `updated_at` and the database holds the newer one, so the next refresh
/// applies the newer row again.
#[derive(Debug, Default)]
pub(crate) struct AppliedRuntime {
    pub models: HashMap<String, DateTime<Utc>>,
    pub providers: HashMap<String, DateTime<Utc>>,
}

/// Keeps the warning for a provider config that cannot be applied to once a
/// minute per provider: one that stays unusable would otherwise repeat at every
/// refresh, and one provider's warning must not hide another's. The map holds
/// at most one entry per provider name.
static NOT_APPLIED: LazyLock<Mutex<HashMap<String, Throttle>>> = LazyLock::new(Default::default);

/// Whether the warning for `provider` may be logged now.
fn not_applied_warning_due(provider: &str) -> bool {
    // A panic while holding the lock leaves a map that is still right.
    let mut throttles = NOT_APPLIED.lock().unwrap_or_else(|p| p.into_inner());
    throttles.entry(provider.to_string()).or_default().due(Instant::now())
}

/// Reload everything the management API stores from the database and apply
/// what differs from what this replica runs, so a change saved on another
/// replica reaches this one (`docs/gateway-settings.md`): the gateway
/// settings, the runtime routes, runtime models, disabled models and provider
/// configs. Whether anything changed. On an error what is in force stays.
/// It also empties the cache's local tier if another replica cleared the
/// cache (`GenerationCache::sync_epoch`), which is not counted as a change.
pub async fn refresh(state: &AppState) -> Result<bool, sqlx::Error> {
    // First, and whatever the database says: a cache cleared on another
    // replica is emptied here too (the event for it can be lost).
    state.router.cache.sync_epoch().await;
    let Some(store) = state.db.management() else { return Ok(false) };
    // Each part runs even when an earlier one changed something.
    let mut changed = sync_gateway(state, store).await?;
    changed |= sync_models(state, store).await?;
    changed |= sync_model_states(state, store).await?;
    changed |= sync_providers(state, store).await?;
    Ok(changed)
}

/// The gateway settings and the runtime routes. Whether they changed.
async fn sync_gateway(state: &AppState, store: &dyn ManagementStore) -> Result<bool, sqlx::Error> {
    // Read before the database: a save that lands during the read wins.
    let version = state.router.overrides_version();
    let gateway = load_gateway_settings(store).await?;
    let model_routes = load_routes(store).await?.map(|r| r.iter().map(route_spec_to_config).collect::<Vec<_>>());
    let current = state.router.overrides();
    if current.gateway == gateway && current.model_routes == model_routes {
        return Ok(false);
    }
    Ok(state.router.set_overrides_unless_changed(version, crate::proxy::router::RuntimeOverrides { model_routes, gateway }))
}

/// The runtime models: a row new or changed since this replica applied it is
/// applied again, and a model whose row is gone leaves the runtime layer.
/// That covers a model this replica saved itself, which the record does not
/// know: the layer is held to the database's rows, not only to the record.
async fn sync_models(state: &AppState, store: &dyn ManagementStore) -> Result<bool, sqlx::Error> {
    // Read before the database, as `sync_gateway` does: a save or delete on
    // this replica that lands during the read wins (the update below is then
    // refused), and the next refresh applies the database, which holds it.
    let version = state.registry.runtime_version();
    // The models served now, taken before the rows are read. A handler writes
    // its row before it serves the model, so a model in this list had its row
    // committed before the read below, and is swept only if the row is gone
    // since. Taken after the read, it could hold a model whose row the read
    // missed, and sweep it until the next tick.
    let served: Vec<String> = state.registry.runtime_models().into_iter().map(|m| m.id).collect();
    let rows = store.list_runtime_models().await?;
    #[cfg(test)]
    state.router.pause_after_sync_read("runtime_models").await;
    let applied = state.router.applied_runtime().models.clone();
    let (mut upserts, mut removals, mut seen) = (Vec::new(), Vec::new(), Vec::new());
    for row in rows.iter().filter(|r| applied.get(&r.id) != Some(&r.updated_at)) {
        match runtime_model_of(state, row) {
            Some(m) => upserts.push(m),
            // A row that does not validate changes what is served only by
            // taking an older version out.
            None => removals.push(row.id.clone()),
        }
        seen.push((row.id.clone(), row.updated_at));
    }
    let stored: HashSet<&str> = rows.iter().map(|r| r.id.as_str()).collect();
    let gone: Vec<String> = applied.keys().cloned().chain(served).filter(|id| !stored.contains(id.as_str())).collect();
    removals.extend(gone.iter().cloned());
    let Some(changed) = state.registry.update_runtime_unless_changed(version, upserts, &removals) else {
        return Ok(false);
    };
    let mut record = state.router.applied_runtime();
    for (id, updated_at) in seen {
        record.models.insert(id, updated_at);
    }
    for id in &gone {
        record.models.remove(id);
    }
    Ok(changed)
}

/// Disabled models: the registry is made to hold the database's set, unless
/// a PATCH on this replica changed it while the database was read. That PATCH
/// stands (the replica skips its own reload event), and the next refresh
/// applies the database, which holds it.
async fn sync_model_states(state: &AppState, store: &dyn ManagementStore) -> Result<bool, sqlx::Error> {
    // Read before the database, as `sync_gateway` does.
    let version = state.registry.disabled_version();
    let stored: HashSet<String> = store.list_model_states().await?.into_iter().filter(|(_, disabled)| *disabled).map(|(id, _)| id).collect();
    #[cfg(test)]
    state.router.pause_after_sync_read("model_states").await;
    Ok(state.registry.replace_disabled_unless_changed(version, stored).unwrap_or(false))
}

/// Provider configs: one whose row is new or changed since this replica applied
/// it, or whose row is gone, goes through [`apply_provider`] again, which also
/// handles the deleted row (the env config, or out of service). A provider
/// this replica configured itself and no row backs any more is swept the same
/// way, for the reason [`sync_models`] gives.
async fn sync_providers(state: &AppState, store: &dyn ManagementStore) -> Result<bool, sqlx::Error> {
    let rows = store.list_provider_configs().await?;
    let applied = state.router.applied_runtime().providers.clone();
    let stored: HashSet<&str> = rows.iter().map(|r| r.provider.as_str()).collect();
    let mut changed = false;

    for row in rows.iter().filter(|r| applied.get(&r.provider) != Some(&r.updated_at)) {
        if reapply_provider(state, store, &row.provider).await {
            state.router.applied_runtime().providers.insert(row.provider.clone(), row.updated_at);
            changed = true;
        }
    }

    let mut gone: Vec<&str> = applied.keys().map(String::as_str).filter(|name| !stored.contains(name)).collect();
    for name in all_provider_names() {
        if !stored.contains(name) && !applied.contains_key(name) && env_config(state, name).is_none() && state.router.registry.registered_config(name).await.is_some() {
            gone.push(name);
        }
    }
    for name in gone {
        if reapply_provider(state, store, name).await {
            state.router.applied_runtime().providers.remove(name);
            changed = true;
        }
    }
    Ok(changed)
}

/// [`apply_provider`] for a refresh. Whether it applied; a failure is logged
/// and tried again at the next refresh.
async fn reapply_provider(state: &AppState, store: &dyn ManagementStore, name: &str) -> bool {
    match apply_provider(state, store, name).await {
        Ok(_) => true,
        Err(e) => {
            if not_applied_warning_due(name) {
                tracing::warn!(provider = %name, error = %e, "runtime provider config not applied; trying again at the next refresh");
            }
            false
        }
    }
}

/// [`refresh`] every `every`, from the next tick (boot loaded everything
/// already), and at once when another replica says it changed something. The
/// listener needs Redis and `subscribe`; without it the tick alone keeps the
/// replicas in step, which takes up to `every` for a change.
pub fn spawn_runtime_sync(state: Arc<AppState>, every: Duration, subscribe: bool) -> tokio::task::JoinHandle<()> {
    let listen = if subscribe { state.router.shared().cloned().map(|shared| (shared, state.router.cache.clone())) } else { None };
    spawn_sync_loop(every, listen, move || {
        let state = state.clone();
        async move { refresh(&state).await }
    })
}

/// [`spawn_runtime_sync`]'s loop, with what one refresh does given by
/// `refresh` (a test passes its own), and Redis's events heard when `listen`
/// names it and the cache a heard clear empties.
fn spawn_sync_loop<R, F>(
    every: Duration,
    listen: Option<(Arc<crate::shared::SharedRedis>, Arc<crate::proxy::cache::GenerationCache>)>,
    mut refresh: R,
) -> tokio::task::JoinHandle<()>
where
    R: FnMut() -> F + Send + 'static,
    F: std::future::Future<Output = Result<bool, sqlx::Error>> + Send,
{
    tokio::spawn(async move {
        let wake = Arc::new(Notify::new());
        // Held for as long as this task runs: dropping the task stops the listener.
        let _listener = listen.map(|(shared, cache)| {
            let wake = wake.clone();
            StopOnDrop(crate::shared::events::spawn_event_listener(shared, move |event| match event {
                SharedEvent::Reload => wake.notify_one(),
                // Not through `wake`: a cache clear needs one read of the epoch,
                // not a reload of everything from the database. Spawned, so the
                // listener goes on reading events while Redis answers.
                SharedEvent::CacheClear => {
                    let cache = cache.clone();
                    tokio::spawn(async move { cache.sync_epoch().await });
                }
            }))
        });
        let mut tick = tokio::time::interval(every);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        tick.tick().await;
        loop {
            tokio::select! {
                _ = tick.tick() => {}
                _ = wake.notified() => {}
            }
            // A refresh that panics is logged by the guard, and the next tick
            // (or event) refreshes again.
            let Some(refreshed) = crate::supervise::guarded("a runtime configuration refresh", refresh()).await else { continue };
            match refreshed {
                Ok(true) => tracing::info!("runtime configuration refreshed from the database; changes applied"),
                Ok(false) => {}
                Err(e) => tracing::warn!(error = %e, "could not refresh the runtime configuration; keeping the current one"),
            }
        }
    })
}

/// Aborts the task when dropped.
struct StopOnDrop(tokio::task::JoinHandle<()>);

impl Drop for StopOnDrop {
    fn drop(&mut self) {
        self.0.abort();
    }
}

/// Tell the other replicas a change was saved, so they refresh now rather than
/// at their next tick. Best effort: with no shared Redis it does nothing, and a
/// failure is logged and never fails the request.
pub async fn announce(state: &AppState, event: SharedEvent) {
    if let Some(shared) = state.router.shared() {
        crate::shared::events::publish_event(shared, event).await;
    }
}

/// Run `work` to its end even if whoever awaits this is dropped: a client that
/// goes away makes the server drop the handler, and a clear of the shared cache
/// stopped half way would leave answers behind with no epoch raised and no
/// event sent. The caller still waits for `work`, so the answer to the client
/// is the same. A panic in `work` is the caller's.
pub async fn finish_even_if_dropped(work: impl std::future::Future<Output = ()> + Send + 'static) {
    finish_even_if_dropped_with(work).await;
}

/// [`finish_even_if_dropped`] for work whose outcome the caller answers with.
/// `None` only when the runtime is shutting down and cancelled the work.
pub async fn finish_even_if_dropped_with<T: Send + 'static>(work: impl std::future::Future<Output = T> + Send + 'static) -> Option<T> {
    match tokio::spawn(work).await {
        Ok(outcome) => Some(outcome),
        // Only a panic gets here, or the runtime shutting down.
        Err(e) => match e.try_into_panic() {
            Ok(panic) => std::panic::resume_unwind(panic),
            Err(_) => None,
        },
    }
}

/// What [`apply_at_boot`] loaded.
#[derive(Debug, Default, PartialEq)]
pub struct BootReport {
    pub runtime_models: usize,
    pub skipped_models: usize,
    pub disabled_models: usize,
    pub provider_configs: usize,
    pub routes_overridden: bool,
}

/// Install one stored runtime model, as boot and [`refresh`] both do: parsed
/// and validated like a model posted to the API. Whether it is now served. A
/// row that does not hold up is logged and left out of the runtime layer, even
/// if an earlier version of it was in.
fn apply_runtime_model(state: &AppState, row: &StoredRuntimeModel) -> bool {
    match runtime_model_of(state, row) {
        Some(m) => {
            state.registry.upsert_runtime(m);
            true
        }
        None => {
            state.registry.remove_runtime(&row.id);
            false
        }
    }
}

/// A stored runtime model, parsed and validated like a model posted to the
/// API. `None`, logged, for a row that does not hold up.
fn runtime_model_of(state: &AppState, row: &StoredRuntimeModel) -> Option<ModelSchema> {
    let parsed: Result<ModelSchema, String> = serde_json::from_str(&row.schema_json).map_err(|e| e.to_string());
    match parsed.and_then(|m| validate_runtime_model(state, &m).map(|_| m)) {
        Ok(m) if m.id == row.id => return Some(m),
        Ok(_) => tracing::warn!(model = %row.id, "runtime model id does not match its schema; skipped"),
        Err(e) => tracing::warn!(model = %row.id, error = %e, "invalid runtime model skipped"),
    }
    None
}

/// Apply everything the management API stored to a freshly built gateway.
/// Invalid rows are logged and skipped; they never stop the server starting.
pub async fn apply_at_boot(state: &AppState) -> Result<BootReport, sqlx::Error> {
    let Some(store) = state.db.management() else { return Ok(BootReport::default()) };
    let mut report = BootReport::default();

    let gateway = load_gateway_settings(store).await?;
    let routes = load_routes(store).await?;
    report.routes_overridden = routes.is_some();
    state.router.set_overrides(crate::proxy::router::RuntimeOverrides {
        model_routes: routes.map(|r| r.iter().map(route_spec_to_config).collect()),
        gateway,
    });

    for row in store.list_runtime_models().await? {
        if apply_runtime_model(state, &row) {
            report.runtime_models += 1;
        } else {
            report.skipped_models += 1;
        }
        state.router.applied_runtime().models.insert(row.id.clone(), row.updated_at);
    }

    for (id, disabled) in store.list_model_states().await? {
        state.registry.set_disabled(&id, disabled);
        if disabled {
            report.disabled_models += 1;
        }
    }

    for row in store.list_provider_configs().await? {
        match apply_provider(state, store, &row.provider).await {
            Ok(_) => {
                report.provider_configs += 1;
                state.router.applied_runtime().providers.insert(row.provider.clone(), row.updated_at);
            }
            Err(e) => tracing::warn!(provider = %row.provider, error = %e, "runtime provider config not applied"),
        }
    }
    Ok(report)
}

/// Collapse a model-mapping/extra-header map for comparisons in tests.
#[cfg(test)]
pub(crate) fn sorted(m: &std::collections::HashMap<String, String>) -> Vec<(String, String)> {
    let mut v: Vec<_> = m.iter().map(|(k, v)| (k.clone(), v.clone())).collect();
    v.sort();
    v
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn each_provider_that_fails_to_apply_gets_its_own_warning_a_minute() {
        // The throttles are process-wide, so these names belong to this test.
        let (a, b) = (format!("test-a-{}", uuid::Uuid::new_v4()), format!("test-b-{}", uuid::Uuid::new_v4()));
        assert!(not_applied_warning_due(&a), "the first failure of a provider warns");
        assert!(!not_applied_warning_due(&a), "and then not again within the minute");
        assert!(not_applied_warning_due(&b), "another provider's failure is not hidden by it");
        assert!(!not_applied_warning_due(&b));
    }

    /// N5 (P11's guard): a refresh that panics is logged, and the sync goes on
    /// at its next tick; it used to end the task, so this replica stopped
    /// taking other replicas' saves until a restart.
    #[tokio::test]
    async fn the_sync_goes_on_after_a_refresh_panicked() {
        use std::sync::atomic::{AtomicUsize, Ordering};
        let refreshes = Arc::new(AtomicUsize::new(0));
        let counted = refreshes.clone();
        let sync = spawn_sync_loop(Duration::from_millis(20), None, move || {
            let n = counted.fetch_add(1, Ordering::SeqCst);
            async move {
                if n == 0 {
                    panic!("a refresh that panics");
                }
                Ok::<bool, sqlx::Error>(false)
            }
        });
        for _ in 0..150 {
            if refreshes.load(Ordering::SeqCst) >= 3 {
                break;
            }
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
        let finished = sync.is_finished();
        sync.abort();
        assert!(refreshes.load(Ordering::SeqCst) >= 3, "the sync stopped at the panic (task finished: {finished})");
    }

    #[tokio::test]
    async fn work_given_to_finish_even_if_dropped_outlives_a_dropped_caller() {
        use std::sync::atomic::{AtomicBool, Ordering};
        let gate = Arc::new(Notify::new());
        let done = Arc::new(AtomicBool::new(false));
        let work = {
            let (gate, done) = (gate.clone(), done.clone());
            async move {
                gate.notified().await;
                done.store(true, Ordering::SeqCst);
            }
        };
        let caller = tokio::spawn(finish_even_if_dropped(work));
        tokio::task::yield_now().await;
        caller.abort();
        let _ = caller.await;
        gate.notify_one();
        for _ in 0..200 {
            if done.load(Ordering::SeqCst) {
                return;
            }
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
        panic!("the work was dropped with its caller");
    }

    fn spec(model: &str, provider: &str) -> RouteSpec {
        RouteSpec {
            model: model.into(),
            deployments: vec![RouteDeployment { provider: provider.into(), weight: 1, max_retries: 2, timeout_seconds: 60, rpm_limit: 0 }],
            strategy: None,
            cache_enabled: None,
            cache_ttl_seconds: None,
        }
    }

    #[test]
    fn routes_round_trip_through_config() {
        let mut r = spec("openai/*", "openai");
        r.strategy = Some("lowest_cost".into());
        r.cache_enabled = Some(true);
        r.cache_ttl_seconds = Some(60);
        assert_eq!(route_config_to_spec(&route_spec_to_config(&r)), r);
    }

    #[test]
    fn route_validation_names_the_problem() {
        assert!(validate_routes(&[spec("openai/*", "openai")]).is_ok());
        assert!(validate_routes(&[spec("", "openai")]).unwrap_err().contains("model"));
        assert!(validate_routes(&[spec("x", "nope")]).unwrap_err().contains("unknown provider 'nope'"));
        let mut r = spec("x", "openai");
        r.strategy = Some("random".into());
        assert!(validate_routes(&[r]).unwrap_err().contains("strategy"));
        let mut r = spec("x", "openai");
        r.deployments.clear();
        assert!(validate_routes(&[r]).unwrap_err().contains("deployment"));
        let mut r = spec("x", "openai");
        r.deployments[0].weight = 0;
        assert!(validate_routes(&[r]).unwrap_err().contains("weight"));
    }

    #[test]
    fn gateway_settings_validation() {
        assert!(validate_gateway_settings(&GatewaySettings::default()).is_ok());
        let bad = |s: GatewaySettings| validate_gateway_settings(&s).unwrap_err();
        assert!(bad(GatewaySettings { cost_markup_percent: Some(-1.0), ..Default::default() }).contains("markup"));
        assert!(bad(GatewaySettings { alert_webhook_url: Some("ftp://x".into()), ..Default::default() }).contains("alert_webhook_url"));
        assert!(bad(GatewaySettings { default_end_user_max_budget_usd: Some(-2.0), ..Default::default() }).contains("default_end_user"));
        assert!(bad(GatewaySettings { default_end_user_budget_duration: Some("soon".into()), ..Default::default() }).contains("budget_duration"));
        assert!(bad(GatewaySettings { storage_access_signed_url_ttl_seconds: Some(59), ..Default::default() }).contains("storage_access_signed_url_ttl_seconds"));
        assert!(bad(GatewaySettings { storage_access_signed_url_ttl_seconds: Some(604_801), ..Default::default() }).contains("604800"));
        assert!(validate_gateway_settings(&GatewaySettings { storage_access_signed_url_ttl_seconds: Some(60), ..Default::default() }).is_ok());
        assert!(bad(GatewaySettings { storage_access_retention_days: Some(36_501), ..Default::default() }).contains("storage_access_retention_days"));
        assert!(validate_gateway_settings(&GatewaySettings { storage_access_retention_days: Some(36_500), ..Default::default() }).is_ok());
        assert!(validate_gateway_settings(&GatewaySettings { storage_access_retention_days: Some(0), ..Default::default() }).is_ok());
        for (s, field) in [
            (GatewaySettings { cache_ttl_seconds: Some(0), ..Default::default() }, "cache_ttl_seconds"),
            (GatewaySettings { cache_max_items: Some(1_000_001), ..Default::default() }, "cache_max_items"),
            (GatewaySettings { cache_max_bytes: Some(1_048_575), ..Default::default() }, "cache_max_bytes"),
            (GatewaySettings { cache_max_bytes: Some(68_719_476_737), ..Default::default() }, "cache_max_bytes"),
            (GatewaySettings { retry_max_retries: Some(6), ..Default::default() }, "retry_max_retries"),
            (GatewaySettings { retry_backoff_ms: Some(60_001), ..Default::default() }, "retry_backoff_ms"),
            (GatewaySettings { retry_backoff_ms: Some(2000), retry_max_backoff_ms: Some(1000), ..Default::default() }, "retry_max_backoff_ms"),
            (GatewaySettings { vendor_timeout_seconds: Some(4), ..Default::default() }, "vendor_timeout_seconds"),
            (GatewaySettings { circuit_breaker_threshold: Some(0), ..Default::default() }, "circuit_breaker_threshold"),
            (GatewaySettings { circuit_breaker_open_for_seconds: Some(3601), ..Default::default() }, "circuit_breaker_open_for_seconds"),
        ] {
            assert!(bad(s).contains(field), "{field}");
        }
        let fine = GatewaySettings {
            cache_enabled: Some(true),
            cache_ttl_seconds: Some(604_800),
            cache_max_items: Some(1),
            cache_max_bytes: Some(68_719_476_736),
            retry_max_retries: Some(0),
            retry_backoff_ms: Some(0),
            retry_max_backoff_ms: Some(60_000),
            vendor_timeout_seconds: Some(3600),
            circuit_breaker_threshold: Some(1000),
            circuit_breaker_open_for_seconds: Some(1),
            ..Default::default()
        };
        assert!(validate_gateway_settings(&fine).is_ok());
    }

    #[test]
    fn effective_config_layers_runtime_over_env() {
        let mut env = blank_env_config();
        env.api_key = Some("env-key".into());
        env.api_base = Some("https://env.example".into());
        env.region = Some("us-east-1".into());
        let secrets = ProviderSecrets { api_key: None, ..Default::default() };
        let mut mapping = std::collections::HashMap::new();
        mapping.insert("a".to_string(), "b".to_string());
        let settings = ProviderSettings {
            api_base: Some("https://runtime.example".into()),
            model_mapping: Some(mapping.clone()),
            enabled: Some(false),
            ..Default::default()
        };
        let c = effective_config(Some(&env), &secrets, &settings);
        assert_eq!(c.api_key.as_deref(), Some("env-key"), "unset secret falls through to env");
        assert_eq!(c.api_base.as_deref(), Some("https://runtime.example"));
        assert_eq!(c.region.as_deref(), Some("us-east-1"));
        assert_eq!(sorted(&c.model_mapping), sorted(&mapping));
        assert!(!c.enabled);

        let c = effective_config(None, &ProviderSecrets { api_key: Some("rt".into()), ..Default::default() }, &ProviderSettings::default());
        assert_eq!(c.api_key.as_deref(), Some("rt"));
        assert!(c.enabled, "no env config defaults to enabled");
    }

    #[test]
    fn key_hints_never_reveal_short_secrets() {
        assert_eq!(key_hint("sk-abcdefgh1234").as_deref(), Some("…1234"));
        assert_eq!(key_hint("short").as_deref(), Some("…"));
    }

    #[test]
    fn provider_names_cover_every_adapter_once() {
        let names = all_provider_names();
        assert!(names.contains(&"openai") && names.contains(&"mock") && names.contains(&"fal"));
        let mut dedup = names.clone();
        dedup.dedup();
        assert_eq!(dedup.len(), names.len());
    }
}
