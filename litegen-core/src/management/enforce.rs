//! Per-request enforcement of management limits on billable generations.
//!
//! [`begin`] runs after the key's own quota reservation (`reserve_quota`) and
//! before dispatch. For every principal the request is attributed to (key,
//! org, app, org member, end user) it rolls a lapsed budget window, applies the
//! principal's model allowlist, rpm limit and parallel-request cap, and
//! reserves the estimated cost against its budget. Any refusal undoes the
//! reservations already taken. The returned [`LimitGuard`] holds the parallel
//! slots until it is dropped and must be [`settle`](LimitGuard::settle)d with
//! the actual cost, exactly like `settle_quota`.

use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use axum::http::{HeaderMap, StatusCode};
use axum::response::{IntoResponse, Response};
use axum::Json;
use chrono::Utc;
use once_cell::sync::Lazy;

use super::budget;
use super::store::ManagementStore;
use super::types::*;
use crate::api::middleware::{AppState, KeyContext};

/// Header naming the end user a request is made on behalf of.
pub const END_USER_HEADER: &str = "x-litegen-end-user";
/// Header carrying comma-separated spend tags for a request.
pub const TAGS_HEADER: &str = "x-litegen-tags";

const MAX_TAGS: usize = 20;
const MAX_TAG_LEN: usize = 64;
const MAX_END_USER_LEN: usize = 256;

/// End user and spend tags a caller attached to a request.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct RequestAttribution {
    pub end_user: Option<String>,
    pub tags: Vec<String>,
}

impl RequestAttribution {
    pub fn from_headers(headers: &HeaderMap) -> Self {
        let end_user = headers
            .get(END_USER_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(str::trim)
            .filter(|s| !s.is_empty())
            .map(|s| s.chars().take(MAX_END_USER_LEN).collect());
        let mut tags: Vec<String> = headers
            .get(TAGS_HEADER)
            .and_then(|v| v.to_str().ok())
            .map(|s| {
                s.split(',')
                    .map(str::trim)
                    .filter(|t| !t.is_empty())
                    .map(|t| t.chars().take(MAX_TAG_LEN).collect::<String>())
                    .collect()
            })
            .unwrap_or_default();
        tags.sort();
        tags.dedup();
        tags.truncate(MAX_TAGS);
        Self { end_user, tags }
    }
}

impl<S: Send + Sync> axum::extract::FromRequestParts<S> for RequestAttribution {
    type Rejection = std::convert::Infallible;

    async fn from_request_parts(parts: &mut axum::http::request::Parts, _state: &S) -> Result<Self, Self::Rejection> {
        Ok(Self::from_headers(&parts.headers))
    }
}

fn refuse(status: StatusCode, kind: &str, message: &str) -> Response {
    (
        status,
        Json(serde_json::json!({ "error": { "message": message, "type": kind, "code": status.as_u16() } })),
    )
        .into_response()
}

fn store_error(e: sqlx::Error) -> Response {
    tracing::error!(error = %e, "limits enforcement failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [("retry-after", "1")],
        Json(serde_json::json!({ "error": {
            "message": "temporarily unable to authorize request", "type": "service_unavailable", "code": 503
        } })),
    )
        .into_response()
}

// ─── Parallel-request slots ──────────────────────────────────────────────────

static IN_FLIGHT: Lazy<Mutex<HashMap<(EntityType, String), u32>>> = Lazy::new(|| Mutex::new(HashMap::new()));

/// One occupied parallel-request slot; freed on drop.
#[derive(Debug)]
pub struct ParallelSlot {
    key: (EntityType, String),
}

impl ParallelSlot {
    pub(crate) fn try_acquire(entity_type: EntityType, entity_id: &str, max: u32) -> Option<Self> {
        let key = (entity_type, entity_id.to_string());
        let mut map = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        let n = map.entry(key.clone()).or_insert(0);
        if *n >= max {
            return None;
        }
        *n += 1;
        Some(Self { key })
    }
}

impl Drop for ParallelSlot {
    fn drop(&mut self) {
        let mut map = IN_FLIGHT.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(n) = map.get_mut(&self.key) {
            *n = n.saturating_sub(1);
            if *n == 0 {
                map.remove(&self.key);
            }
        }
    }
}

/// How many requests a principal currently has in flight.
pub fn in_flight(entity_type: EntityType, entity_id: &str) -> u32 {
    IN_FLIGHT
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .get(&(entity_type, entity_id.to_string()))
        .copied()
        .unwrap_or(0)
}

/// Stable rate-limiter bucket id for a principal, disjoint from key ids.
fn bucket_id(entity_type: EntityType, entity_id: &str) -> uuid::Uuid {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(format!("limits:{}:{}", entity_type.as_str(), entity_id).as_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    uuid::Uuid::from_bytes(bytes)
}

// ─── Model access ────────────────────────────────────────────────────────────

/// Whether the org's model pool and the app's access mode let a request use
/// `model_id`. An empty org pool means the org is unrestricted; app mode `all`
/// inherits the org pool, `select` allows only its listed models.
pub async fn tenant_allows_model(
    state: &AppState,
    org_id: Option<&str>,
    app_id: Option<&str>,
    model_id: &str,
) -> Result<bool, sqlx::Error> {
    if let Some(org) = org_id {
        let pool = state.db.get_org_allowed_models(org).await?;
        if !pool.is_empty() && !pool.iter().any(|m| m == model_id) {
            return Ok(false);
        }
    }
    if let Some(app) = app_id {
        let (mode, models) = state.db.get_app_model_access(app).await?;
        if mode == "select" && !models.iter().any(|m| m == model_id) {
            return Ok(false);
        }
    }
    Ok(true)
}

/// The model ids a tenant may use, or None when it is unrestricted. Same rules
/// as [`tenant_allows_model`], computed once for listing.
pub async fn tenant_model_filter(
    state: &AppState,
    org_id: Option<&str>,
    app_id: Option<&str>,
) -> Result<Option<std::collections::HashSet<String>>, sqlx::Error> {
    let mut allowed: Option<std::collections::HashSet<String>> = None;
    if let Some(org) = org_id {
        let pool = state.db.get_org_allowed_models(org).await?;
        if !pool.is_empty() {
            allowed = Some(pool.into_iter().collect());
        }
    }
    if let Some(app) = app_id {
        let (mode, models) = state.db.get_app_model_access(app).await?;
        if mode == "select" {
            let select: std::collections::HashSet<String> = models.into_iter().collect();
            allowed = Some(match allowed {
                Some(pool) => pool.intersection(&select).cloned().collect(),
                None => select,
            });
        }
    }
    Ok(allowed)
}

// ─── Window handling ─────────────────────────────────────────────────────────

/// Load a principal's limits, rolling its budget window first if it lapsed.
pub async fn load_current(
    store: &dyn ManagementStore,
    entity_type: EntityType,
    entity_id: &str,
) -> Result<Option<EntityLimits>, sqlx::Error> {
    let Some(mut limits) = store.get_limits(entity_type, entity_id).await? else { return Ok(None) };
    let now = Utc::now();
    if let Some(next) = budget::lapsed_window_next_reset(limits.budget_duration.as_deref(), limits.budget_reset_at, now) {
        if store.roll_limits_window(entity_type, entity_id, limits.budget_reset_at, Some(next)).await? {
            limits.spend_usd = 0.0;
            limits.soft_budget_alerted_at = None;
            limits.budget_reset_at = Some(next);
        } else if let Some(fresh) = store.get_limits(entity_type, entity_id).await? {
            // Another request rolled it first.
            limits = fresh;
        }
    }
    Ok(Some(limits))
}

// ─── Guard ───────────────────────────────────────────────────────────────────

/// Reservations and parallel slots held for one generation.
#[derive(Debug, Default)]
pub struct LimitGuard {
    store_backed: bool,
    reserved: f64,
    charged: Vec<EntityLimits>,
    attribution: Attribution,
    _slots: Vec<ParallelSlot>,
}

impl LimitGuard {
    /// Who the request is attributed to, for its request-log row.
    pub fn attribution(&self) -> Attribution {
        self.attribution.clone()
    }

    /// Settle every reservation to `actual` (0.0 releases them), then raise a
    /// soft-budget alert for any principal this request pushed over its
    /// threshold. Call exactly once.
    pub async fn settle(&mut self, state: &AppState, actual: f64) {
        if !self.store_backed {
            return;
        }
        let Some(store) = state.db.management() else { return };
        let delta = actual - self.reserved;
        let charged = std::mem::take(&mut self.charged);
        for limits in charged {
            let spend = if delta.abs() > f64::EPSILON {
                match store.charge_limits_spend(limits.entity_type, &limits.entity_id, delta).await {
                    Ok(s) => s,
                    Err(e) => {
                        tracing::warn!(error = %e, entity = %limits.entity_id, "limits settlement failed");
                        None
                    }
                }
            } else {
                Some(limits.spend_usd + self.reserved)
            };
            if let (Some(spend), Some(soft)) = (spend, limits.soft_budget_usd) {
                if actual > 0.0 && spend >= soft && limits.soft_budget_alerted_at.is_none() {
                    raise_soft_budget_alert(state, store, &limits, spend, soft).await;
                }
            }
        }
        self.reserved = 0.0;
    }
}

async fn raise_soft_budget_alert(
    state: &AppState,
    store: &dyn ManagementStore,
    limits: &EntityLimits,
    spend: f64,
    soft: f64,
) {
    let now = Utc::now();
    match store.mark_soft_budget_alerted(limits.entity_type, &limits.entity_id, now).await {
        Ok(true) => {}
        Ok(false) => return, // another request alerted this window already
        Err(e) => {
            tracing::warn!(error = %e, "failed to record soft budget alert");
            return;
        }
    }
    let payload = serde_json::json!({
        "type": "budget.soft_limit_reached",
        "entity_type": limits.entity_type,
        "entity_id": limits.entity_id,
        "org_id": limits.org_id,
        "spend_usd": spend,
        "soft_budget_usd": soft,
        "max_budget_usd": limits.max_budget_usd,
        "budget_reset_at": limits.budget_reset_at,
        "at": now,
    });
    let entry = crate::types::AuditLogEntry {
        id: format!("audit-{}", uuid::Uuid::new_v4()),
        actor_key_id: None,
        actor_label: "system".to_string(),
        action: "budget.soft_limit_reached".to_string(),
        target_type: format!("{}_limits", limits.entity_type.as_str()),
        target_id: limits.entity_id.clone(),
        before_json: None,
        after_json: Some(payload.to_string()),
        created_at: now,
        org_id: limits.org_id.clone(),
    };
    if let Err(e) = state.db.insert_audit_log(&entry).await {
        tracing::warn!(error = %e, "failed to audit soft budget alert");
    }
    super::runtime::send_alert(state, payload).await;
}

/// Attribution with no enforcement: what `begin` returns when the database
/// has no management store.
fn passthrough(ctx: Option<&KeyContext>, req: &RequestAttribution) -> LimitGuard {
    LimitGuard {
        attribution: Attribution {
            key_id: ctx.and_then(|c| c.key_id).map(|k| k.to_string()),
            user_id: ctx.and_then(|c| c.user.as_ref()).map(|u| u.user_id.clone()),
            end_user: req.end_user.clone(),
            tags: req.tags.clone(),
        },
        ..Default::default()
    }
}

/// Apply every principal's limits to a generation of `model_id` estimated at
/// `estimate` USD. See the module docs.
pub async fn begin(
    state: &AppState,
    ctx: Option<&KeyContext>,
    req: &RequestAttribution,
    model_id: &str,
    estimate: f64,
) -> Result<LimitGuard, Response> {
    let org_id = ctx.and_then(|c| c.org_id.clone());
    let app_id = ctx.and_then(|c| c.app_id.clone());

    if !tenant_allows_model(state, org_id.as_deref(), app_id.as_deref(), model_id).await.map_err(store_error)? {
        return Err(refuse(
            StatusCode::FORBIDDEN,
            "model_not_allowed",
            &format!("model '{model_id}' is not enabled for this organization or application"),
        ));
    }

    let Some(store) = state.db.management() else { return Ok(passthrough(ctx, req)) };

    // The member a request is billed to: the session user, or the user who owns
    // the API key.
    let key_id = ctx.and_then(|c| c.key_id);
    let mut user_id = ctx.and_then(|c| c.user.as_ref()).map(|u| u.user_id.clone());
    if user_id.is_none() {
        if let Some(k) = key_id {
            user_id = state.db.get_api_key(&k).await.map_err(store_error)?.and_then(|k| k.owner_user_id);
        }
    }

    // End user: refuse a blocked one, record first sight / last seen.
    if let (Some(org), Some(eu)) = (org_id.as_deref(), req.end_user.as_deref()) {
        if let Some(row) = store.get_end_user(org, eu).await.map_err(store_error)? {
            if row.blocked {
                return Err(refuse(StatusCode::FORBIDDEN, "end_user_blocked", &format!("end user '{eu}' is blocked")));
            }
        }
        store.touch_end_user(org, eu, Utc::now()).await.map_err(store_error)?;
        super::runtime::ensure_default_end_user_limits(state, store, org, eu).await.map_err(store_error)?;
    }

    let mut principals: Vec<(EntityType, String)> = Vec::new();
    if let Some(k) = key_id {
        principals.push((EntityType::Key, k.to_string()));
    }
    if let Some(o) = &org_id {
        principals.push((EntityType::Org, o.clone()));
        if let Some(u) = &user_id {
            principals.push((EntityType::Member, member_entity_id(o, u)));
        }
        if let Some(eu) = &req.end_user {
            principals.push((EntityType::EndUser, end_user_entity_id(o, eu)));
        }
    }
    if let Some(a) = &app_id {
        principals.push((EntityType::App, a.clone()));
    }

    let mut guard = LimitGuard {
        store_backed: true,
        attribution: Attribution {
            key_id: key_id.map(|k| k.to_string()),
            user_id,
            end_user: req.end_user.clone(),
            tags: req.tags.clone(),
        },
        ..Default::default()
    };

    let mut rows = Vec::new();
    for (t, id) in &principals {
        if let Some(limits) = load_current(store, *t, id).await.map_err(store_error)? {
            rows.push(limits);
        }
    }

    // Cheap, side-effect-free checks first.
    for l in &rows {
        if let Some(allowed) = &l.allowed_models {
            if !allowed.iter().any(|m| m == model_id) {
                return Err(refuse(
                    StatusCode::FORBIDDEN,
                    "model_not_allowed",
                    &format!("model '{model_id}' is not in the {} allowlist", l.entity_type.as_str()),
                ));
            }
        }
    }
    for l in &rows {
        if let Some(max) = l.max_parallel_requests {
            match ParallelSlot::try_acquire(l.entity_type, &l.entity_id, max) {
                Some(slot) => guard._slots.push(slot),
                None => {
                    return Err(refuse(
                        StatusCode::TOO_MANY_REQUESTS,
                        "parallel_limit_exceeded",
                        &format!("{} has {max} requests in flight already", l.entity_type.as_str()),
                    ))
                }
            }
        }
        if let Some(rpm) = l.rpm_limit.filter(|r| *r > 0) {
            if let Err(retry) = state.rate_limiter.try_take(bucket_id(l.entity_type, &l.entity_id), rpm).await {
                let mut resp = refuse(
                    StatusCode::TOO_MANY_REQUESTS,
                    "rate_limit_exceeded",
                    &format!("{} rate limit of {rpm} requests/minute exceeded", l.entity_type.as_str()),
                );
                if let Ok(v) = axum::http::HeaderValue::from_str(&retry.to_string()) {
                    resp.headers_mut().insert("retry-after", v);
                }
                return Err(resp);
            }
        }
    }

    // Reserve the estimate on every row (rows without a cap still track spend).
    let amount = estimate.max(0.0);
    for l in rows {
        if amount > 0.0 {
            match store.reserve_limits_spend(l.entity_type, &l.entity_id, amount).await {
                Ok(true) => {}
                Ok(false) => {
                    guard.reserved = amount;
                    guard.settle(state, 0.0).await;
                    let mut msg = format!("{} budget exceeded", l.entity_type.as_str());
                    if let Some(at) = l.budget_reset_at {
                        msg.push_str(&format!("; resets at {}", at.to_rfc3339()));
                    }
                    return Err(refuse(StatusCode::PAYMENT_REQUIRED, "budget_exceeded", &msg));
                }
                Err(e) => {
                    guard.reserved = amount;
                    guard.settle(state, 0.0).await;
                    return Err(store_error(e));
                }
            }
        }
        guard.charged.push(l);
    }
    guard.reserved = amount;
    Ok(guard)
}

/// Stamp the request-log row `request_id` with the guard's attribution.
/// Failures are logged, never surfaced: attribution must not fail a request.
pub async fn attribute(db: &Arc<dyn crate::db::DatabaseStore>, request_id: &str, attribution: &Attribution) {
    let Some(store) = db.management() else { return };
    if attribution == &Attribution::default() {
        return;
    }
    if let Err(e) = store.attribute_request_log(request_id, attribution).await {
        tracing::warn!(error = %e, request_id, "failed to attribute request log");
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn attribution_headers_are_trimmed_deduped_and_capped() {
        let mut h = HeaderMap::new();
        h.insert(END_USER_HEADER, "  cust-42 ".parse().unwrap());
        h.insert(TAGS_HEADER, "b, a ,,a, c".parse().unwrap());
        let a = RequestAttribution::from_headers(&h);
        assert_eq!(a.end_user.as_deref(), Some("cust-42"));
        assert_eq!(a.tags, vec!["a", "b", "c"]);

        let many: Vec<String> = (0..50).map(|i| format!("t{i:02}")).collect();
        let mut h = HeaderMap::new();
        h.insert(TAGS_HEADER, many.join(",").parse().unwrap());
        assert_eq!(RequestAttribution::from_headers(&h).tags.len(), MAX_TAGS);
    }

    #[test]
    fn missing_or_blank_headers_mean_no_attribution() {
        let mut h = HeaderMap::new();
        h.insert(END_USER_HEADER, "   ".parse().unwrap());
        assert_eq!(RequestAttribution::from_headers(&h), RequestAttribution::default());
        assert_eq!(RequestAttribution::from_headers(&HeaderMap::new()), RequestAttribution::default());
    }

    #[test]
    fn parallel_slots_cap_and_free_on_drop() {
        let id = format!("slot-test-{}", uuid::Uuid::new_v4());
        let a = ParallelSlot::try_acquire(EntityType::Key, &id, 2).unwrap();
        let b = ParallelSlot::try_acquire(EntityType::Key, &id, 2).unwrap();
        assert!(ParallelSlot::try_acquire(EntityType::Key, &id, 2).is_none());
        assert_eq!(in_flight(EntityType::Key, &id), 2);
        drop(a);
        assert_eq!(in_flight(EntityType::Key, &id), 1);
        let _c = ParallelSlot::try_acquire(EntityType::Key, &id, 2).unwrap();
        drop(b);
        assert_eq!(in_flight(EntityType::Key, &id), 1);
    }

    #[test]
    fn bucket_ids_are_stable_and_distinct_per_entity() {
        assert_eq!(bucket_id(EntityType::Org, "o1"), bucket_id(EntityType::Org, "o1"));
        assert_ne!(bucket_id(EntityType::Org, "o1"), bucket_id(EntityType::App, "o1"));
        assert_ne!(bucket_id(EntityType::Org, "o1"), bucket_id(EntityType::Org, "o2"));
    }
}
