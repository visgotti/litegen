//! Runtime configuration: loading what the management API stored at boot, and
//! the helpers that apply a change to the live gateway.
//!
//! - Gateway settings and model routes live on the router
//!   ([`ProxyRouter::set_overrides`](crate::proxy::router::ProxyRouter::set_overrides)).
//! - Runtime models and disabled ids live on the capability registry's runtime
//!   layer, so the validator and `/v1/models` see them immediately.
//! - Provider configs rebuild the provider instance in the provider registry.
//!   A job already in flight keeps the instance it submitted with.


use chrono::Utc;

use super::store::ManagementStore;
use super::types::*;
use crate::api::middleware::AppState;
use crate::capabilities::schema::{MediaType, ModelSchema};
use crate::config::{CacheRouteConfig, DeploymentConfig, ModelRouteConfig, ProviderEnvConfig};
use crate::proxy::registry::{IMAGE_PROVIDERS, MODEL3D_PROVIDERS, VIDEO_PROVIDERS};

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

// ─── Boot ────────────────────────────────────────────────────────────────────

/// What [`apply_at_boot`] loaded.
#[derive(Debug, Default, PartialEq)]
pub struct BootReport {
    pub runtime_models: usize,
    pub skipped_models: usize,
    pub disabled_models: usize,
    pub provider_configs: usize,
    pub routes_overridden: bool,
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
        let parsed: Result<ModelSchema, String> = serde_json::from_str(&row.schema_json).map_err(|e| e.to_string());
        match parsed.and_then(|m| validate_runtime_model(state, &m).map(|_| m)) {
            Ok(m) if m.id == row.id => {
                state.registry.upsert_runtime(m);
                report.runtime_models += 1;
            }
            Ok(_) => {
                tracing::warn!(model = %row.id, "runtime model id does not match its schema; skipped");
                report.skipped_models += 1;
            }
            Err(e) => {
                tracing::warn!(model = %row.id, error = %e, "invalid runtime model skipped");
                report.skipped_models += 1;
            }
        }
    }

    for (id, disabled) in store.list_model_states().await? {
        state.registry.set_disabled(&id, disabled);
        if disabled {
            report.disabled_models += 1;
        }
    }

    let names: Vec<String> = store.list_provider_configs().await?.into_iter().map(|c| c.provider).collect();
    for name in names {
        match apply_provider(state, store, &name).await {
            Ok(_) => report.provider_configs += 1,
            Err(e) => tracing::warn!(provider = %name, error = %e, "runtime provider config not applied"),
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
