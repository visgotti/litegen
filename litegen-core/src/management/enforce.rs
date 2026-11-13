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
use serde::{Deserialize, Serialize};

use super::budget;
use super::store::ManagementStore;
use super::types::*;
use crate::api::middleware::rate_limit::{unverifiable_response, LimitRefusal};
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
        Json(serde_json::json!({ "error": { "message": message, "type": kind, "code": kind } })),
    )
        .into_response()
}

/// The answer for a principal whose requests-per-minute bucket said no: the
/// 429 with `Retry-After`, or the fixed 503 when fail-closed could not check.
fn rate_refusal(entity: EntityType, rpm: u32, refusal: LimitRefusal) -> Response {
    let retry = match refusal {
        LimitRefusal::Exceeded { retry_after_secs } => retry_after_secs,
        LimitRefusal::Unverifiable => return unverifiable_response(),
    };
    let mut resp = refuse(
        StatusCode::TOO_MANY_REQUESTS,
        "rate_limit_exceeded",
        &format!("{} rate limit of {rpm} requests/minute exceeded", entity.as_str()),
    );
    if let Ok(v) = axum::http::HeaderValue::from_str(&retry.to_string()) {
        resp.headers_mut().insert("retry-after", v);
    }
    resp
}

fn store_error(e: sqlx::Error) -> Response {
    tracing::error!(error = %e, "limits enforcement failed");
    (
        StatusCode::SERVICE_UNAVAILABLE,
        [("retry-after", "1")],
        Json(serde_json::json!({ "error": {
            "message": "temporarily unable to authorize request", "type": "service_unavailable", "code": "service_unavailable"
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

// The slot machinery (`SlotGuard`, `SharedSlot`, `acquire_slot`) lives in `slots`;
// these re-exports keep the paths `enforce::SlotGuard` and `enforce::acquire_slot`.
pub use super::slots::{acquire_slot, SharedSlot, SlotGuard};

/// The answer for a principal whose parallel cap said no: today's 429, with no
/// `Retry-After`, or the fixed 503 when fail-closed could not check.
fn parallel_refusal(entity: EntityType, max: u32, refusal: LimitRefusal) -> Response {
    match refusal {
        LimitRefusal::Exceeded { .. } => refuse(
            StatusCode::TOO_MANY_REQUESTS,
            "parallel_limit_exceeded",
            &format!("{} has {max} requests in flight already", entity.as_str()),
        ),
        LimitRefusal::Unverifiable => unverifiable_response(),
    }
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

/// Reservations and parallel slots held for one generation. Settle it once;
/// a guard dropped unsettled (its handler panicked) gives its reservations
/// back by itself.
#[derive(Debug, Default)]
pub struct LimitGuard {
    store_backed: bool,
    reserved: f64,
    charged: Vec<EntityLimits>,
    attribution: Attribution,
    _slots: Vec<SlotGuard>,
    release_to: ReleaseTo,
}

/// The store an unsettled guard gives its reservations back to when dropped.
#[derive(Default)]
struct ReleaseTo(Option<Arc<dyn crate::db::DatabaseStore>>);

impl std::fmt::Debug for ReleaseTo {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(if self.0.is_some() { "ReleaseTo(store)" } else { "ReleaseTo(none)" })
    }
}

impl Drop for LimitGuard {
    /// What was reserved and never settled goes back, as `settle(0.0)` would
    /// (no soft-budget alert: nothing was charged). In a task of its own,
    /// since a drop cannot await. The slots free themselves.
    fn drop(&mut self) {
        if !self.store_backed || self.reserved <= 0.0 || self.charged.is_empty() {
            return;
        }
        let Some(db) = self.release_to.0.take() else { return };
        let Ok(runtime) = tokio::runtime::Handle::try_current() else {
            tracing::warn!(reserved = self.reserved, "a dropped limits reservation could not be given back outside a runtime");
            return;
        };
        let (reserved, charged) = (std::mem::take(&mut self.reserved), std::mem::take(&mut self.charged));
        runtime.spawn(async move {
            let Some(store) = db.management() else { return };
            for l in charged {
                if let Err(e) = store.charge_limits_spend(l.entity_type, &l.entity_id, -reserved).await {
                    tracing::warn!(error = %e, entity = %l.entity_id, "releasing a dropped limits reservation failed");
                }
            }
        });
    }
}

/// A principal whose limits a generation was charged against: what its
/// refund gives back to (L18).
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct ChargedLimit {
    pub entity_type: EntityType,
    pub entity_id: String,
}

impl LimitGuard {
    /// A guard holding nothing yet. With a store, it enforces limits, and
    /// gives back what it reserved if dropped unsettled; without one, it
    /// only attributes.
    fn new(store: Option<Arc<dyn crate::db::DatabaseStore>>, attribution: Attribution) -> Self {
        LimitGuard {
            store_backed: store.is_some(),
            reserved: 0.0,
            charged: Vec::new(),
            attribution,
            _slots: Vec::new(),
            release_to: ReleaseTo(store),
        }
    }

    /// Who the request is attributed to, for its request-log row.
    pub fn attribution(&self) -> Attribution {
        self.attribution.clone()
    }

    /// The principals this request is charged against. An async job records
    /// them on its row at submit, since its refund comes long after the guard
    /// is gone.
    pub fn charged_to(&self) -> Vec<ChargedLimit> {
        self.charged
            .iter()
            .map(|l| ChargedLimit { entity_type: l.entity_type, entity_id: l.entity_id.clone() })
            .collect()
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
    LimitGuard::new(
        None,
        Attribution {
            key_id: ctx.and_then(|c| c.key_id).map(|k| k.to_string()),
            user_id: ctx.and_then(|c| c.user.as_ref()).map(|u| u.user_id.clone()),
            end_user: req.end_user.clone(),
            tags: req.tags.clone(),
        },
    )
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

    let mut guard = LimitGuard::new(
        Some(state.db.clone()),
        Attribution { key_id: key_id.map(|k| k.to_string()), user_id, end_user: req.end_user.clone(), tags: req.tags.clone() },
    );

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
            match acquire_slot(&state.rate_limiter, l.entity_type, &l.entity_id, max).await {
                Ok(slot) => guard._slots.push(slot),
                Err(refusal) => return Err(parallel_refusal(l.entity_type, max, refusal)),
            }
        }
        if let Some(rpm) = l.rpm_limit.filter(|r| *r > 0) {
            if let Err(refusal) = state.rate_limiter.try_take(bucket_id(l.entity_type, &l.entity_id), rpm).await {
                return Err(rate_refusal(l.entity_type, rpm, refusal));
            }
        }
    }

    // Reserve the estimate on every row (rows without a cap still track spend).
    // The guard counts the amount before the first reservation: `charged`
    // holds only the rows actually reserved, so a guard dropped inside this
    // loop (its task cut off, or a store call that panicked) gives back
    // exactly those (handlers review #10).
    let amount = estimate.max(0.0);
    guard.reserved = amount;
    for l in rows {
        if amount > 0.0 {
            match store.reserve_limits_spend(l.entity_type, &l.entity_id, amount).await {
                Ok(true) => {}
                Ok(false) => {
                    guard.settle(state, 0.0).await;
                    let mut msg = format!("{} budget exceeded", l.entity_type.as_str());
                    if let Some(at) = l.budget_reset_at {
                        msg.push_str(&format!("; resets at {}", at.to_rfc3339()));
                    }
                    return Err(refuse(StatusCode::PAYMENT_REQUIRED, "budget_exceeded", &msg));
                }
                Err(e) => {
                    guard.settle(state, 0.0).await;
                    return Err(store_error(e));
                }
            }
        }
        guard.charged.push(l);
    }
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

/// Give `amount` back to every principal a failed or cancelled job was
/// charged against (L18). Best-effort, like settlement: a refund that cannot
/// be written is logged, never surfaced.
pub async fn refund_limits(db: &Arc<dyn crate::db::DatabaseStore>, charged: &[ChargedLimit], amount: f64) {
    let Some(store) = db.management() else { return };
    for l in charged {
        if let Err(e) = store.charge_limits_spend(l.entity_type, &l.entity_id, -amount).await {
            tracing::warn!(error = %e, entity = %l.entity_id, "limits refund failed");
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::api::middleware::rate_limit::RateLimiter;
    use crate::shared::testing::{dead_redis, two_shared_limiters};
    use std::time::{Duration, Instant};

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

    #[tokio::test]
    async fn a_rate_refusal_is_a_429_with_retry_after_or_the_fixed_503() {
        use crate::api::middleware::rate_limit::LimitRefusal;

        let resp = rate_refusal(EntityType::Org, 60, LimitRefusal::Exceeded { retry_after_secs: 3 });
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(resp.headers().get("retry-after").unwrap(), "3");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "rate_limit_exceeded", "{v}");
        assert_eq!(v["error"]["message"], "org rate limit of 60 requests/minute exceeded", "{v}");

        let resp = rate_refusal(EntityType::Org, 60, LimitRefusal::Unverifiable);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers().get("retry-after").unwrap(), "1");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "rate_limit_unverifiable", "{v}");
    }

    // ─── Parallel refusals, and `begin` over shared slots ─────────────────────
    //
    // The Redis ones need `LITEGEN_REDIS_TEST_URL` and skip themselves without
    // it; the ones on `dead_redis` need no container. The slots themselves are
    // tested in `slots`.

    #[tokio::test]
    async fn a_parallel_refusal_is_todays_429_or_the_fixed_503() {
        let resp = parallel_refusal(EntityType::Key, 3, LimitRefusal::Exceeded { retry_after_secs: 0 });
        assert_eq!(resp.status(), StatusCode::TOO_MANY_REQUESTS);
        assert!(resp.headers().get("retry-after").is_none(), "today's 429 has no Retry-After");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(
            v,
            serde_json::json!({ "error": {
                "message": "key has 3 requests in flight already",
                "type": "parallel_limit_exceeded",
                "code": "parallel_limit_exceeded"
            } })
        );

        let resp = parallel_refusal(EntityType::Key, 3, LimitRefusal::Unverifiable);
        assert_eq!(resp.status(), StatusCode::SERVICE_UNAVAILABLE);
        assert_eq!(resp.headers().get("retry-after").unwrap(), "1");
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(v["error"]["code"], "rate_limit_unverifiable", "{v}");
    }

    struct NoopStorage;
    #[async_trait::async_trait]
    impl crate::proxy::materializer::TempStorage for NoopStorage {
        async fn put(&self, key: &str, _bytes: bytes::Bytes, _ct: &str) -> Result<String, crate::proxy::materializer::MaterializeError> {
            Ok(format!("local://{key}"))
        }
        async fn delete(&self, _key: &str) -> Result<(), crate::proxy::materializer::MaterializeError> {
            Ok(())
        }
    }

    /// A replica: its own limiter over `db`, which holds the principals' limits.
    async fn replica(db: Arc<crate::db::sqlite::SqliteDatabase>, limiter: Arc<RateLimiter>) -> AppState {
        use crate::config::{AppConfig, CacheGlobalConfig};
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        let cache = Arc::new(crate::proxy::cache::GenerationCache::new(&CacheGlobalConfig::default()));
        let router = Arc::new(crate::proxy::router::ProxyRouter::new(
            registry,
            cache,
            Arc::new(AppConfig::default()),
            Arc::new(crate::proxy::storage::LocalStore),
        ));
        let materializer = Arc::new(crate::proxy::materializer::Materializer::new(Arc::new(NoopStorage), reqwest::Client::new()));
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        AppState {
            router,
            db,
            master_key: None,
            registry: Arc::new(crate::capabilities::CapabilityRegistry::from_dir(&models).expect("models")),
            materializer,
            rate_limiter: limiter,
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        }
    }

    /// A key capped at one request in flight, and the context it calls with.
    async fn capped_key(db: &crate::db::sqlite::SqliteDatabase) -> KeyContext {
        use crate::db::DatabaseStore;
        let key_id = uuid::Uuid::new_v4();
        let now = Utc::now();
        db.management()
            .expect("sqlite has a management store")
            .upsert_limits(&EntityLimits {
                entity_type: EntityType::Key,
                entity_id: key_id.to_string(),
                org_id: None,
                max_budget_usd: None,
                soft_budget_usd: None,
                budget_duration: None,
                spend_usd: 0.0,
                budget_reset_at: None,
                soft_budget_alerted_at: None,
                allowed_models: None,
                max_parallel_requests: Some(1),
                rpm_limit: None,
                metadata: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        KeyContext {
            key_id: Some(key_id),
            scopes: vec![],
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: None,
            permissions: vec![],
            session_id: None,
            org_id: None,
            app_id: None,
            oauth_grant_id: None,
        }
    }

    async fn error_code(resp: Response) -> (StatusCode, String) {
        let status = resp.status();
        let body = axum::body::to_bytes(resp.into_body(), 1 << 16).await.unwrap();
        let v: serde_json::Value = serde_json::from_slice(&body).unwrap();
        (status, v["error"]["code"].as_str().unwrap_or_default().to_string())
    }

    #[tokio::test]
    async fn begin_counts_parallel_requests_across_replicas() {
        let Some((a, b)) = two_shared_limiters().await else { return };
        let db = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let ctx = capped_key(&db).await;
        let (on_a, on_b) = (replica(db.clone(), a).await, replica(db, b).await);
        let none = RequestAttribution::default();

        let first = begin(&on_a, Some(&ctx), &none, "mock/image-gen", 0.0).await.unwrap();
        // The local counters are one per process, which these two "replicas"
        // share: a slot counted there would refuse on B for the wrong reason.
        assert_eq!(in_flight(EntityType::Key, &ctx.key_id.unwrap().to_string()), 0, "counted in Redis, not here");
        let refused = begin(&on_b, Some(&ctx), &none, "mock/image-gen", 0.0).await.unwrap_err();
        assert_eq!(error_code(refused).await, (StatusCode::TOO_MANY_REQUESTS, "parallel_limit_exceeded".to_string()));

        drop(first);
        // The release is a task the drop spawns: the slot is back a moment later.
        let deadline = Instant::now() + Duration::from_secs(2);
        while begin(&on_b, Some(&ctx), &none, "mock/image-gen", 0.0).await.is_err() {
            assert!(Instant::now() < deadline, "the slot did not come back within 2 s");
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    }

    #[tokio::test]
    async fn begin_answers_the_fixed_503_when_fail_closed_cannot_check_a_cap() {
        let limiter = Arc::new(RateLimiter::with_shared(dead_redis(true).await));
        let db = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let ctx = capped_key(&db).await;
        let state = replica(db, limiter).await;
        let refused = begin(&state, Some(&ctx), &RequestAttribution::default(), "mock/image-gen", 0.0).await.unwrap_err();
        assert_eq!(error_code(refused).await, (StatusCode::SERVICE_UNAVAILABLE, "rate_limit_unverifiable".to_string()));
    }

    /// The key's tracked spend, as the store has it now.
    async fn key_spend(db: &crate::db::sqlite::SqliteDatabase, ctx: &KeyContext) -> f64 {
        use crate::db::DatabaseStore;
        let key = ctx.key_id.unwrap().to_string();
        db.management().unwrap().get_limits(EntityType::Key, &key).await.unwrap().unwrap().spend_usd
    }

    /// E12: a guard dropped before it was settled (a billed handler that
    /// panicked) gives its reservations back, as `settle(0.0)` would. It used
    /// to leave them held until the principal's budget window rolled.
    #[tokio::test]
    async fn an_unsettled_guard_releases_its_reservations_when_dropped() {
        let db = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let ctx = capped_key(&db).await;
        let state = replica(db.clone(), Arc::new(RateLimiter::new())).await;
        let guard = begin(&state, Some(&ctx), &RequestAttribution::default(), "mock/image-gen", 0.5).await.unwrap();
        assert_eq!(key_spend(&db, &ctx).await, 0.5, "the estimate is reserved");
        drop(guard);
        // The release is a task the drop spawns.
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut left = key_spend(&db, &ctx).await;
        while left != 0.0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            left = key_spend(&db, &ctx).await;
        }
        assert_eq!(left, 0.0, "a guard dropped unsettled left {left} USD reserved");
    }

    /// `entity_type`/`id`'s tracked spend, as the store has it now.
    async fn spend_of(db: &crate::db::sqlite::SqliteDatabase, entity_type: EntityType, id: &str) -> f64 {
        use crate::db::DatabaseStore;
        db.management().unwrap().get_limits(entity_type, id).await.unwrap().unwrap().spend_usd
    }

    /// Handlers review #10: a request cut off inside `begin()`'s reserve loop
    /// (its task aborted, or the store call it waits on panicked) gives back
    /// what it had already reserved. The guard counted its reservation only
    /// after the loop, so dropped inside it, it released nothing, and the
    /// key's reservation stayed held until its budget window rolled.
    #[tokio::test]
    async fn a_request_cut_off_between_two_reservations_gives_back_the_first() {
        use crate::db::DatabaseStore;
        let db = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let mut ctx = capped_key(&db).await;
        let app = format!("app-{}", uuid::Uuid::new_v4());
        let now = Utc::now();
        db.management()
            .unwrap()
            .upsert_limits(&EntityLimits {
                entity_type: EntityType::App,
                entity_id: app.clone(),
                org_id: None,
                max_budget_usd: None,
                soft_budget_usd: None,
                budget_duration: None,
                spend_usd: 0.0,
                budget_reset_at: None,
                soft_budget_alerted_at: None,
                allowed_models: None,
                max_parallel_requests: None,
                rpm_limit: None,
                metadata: None,
                created_at: now,
                updated_at: now,
            })
            .await
            .unwrap();
        ctx.app_id = Some(app.clone());
        let key = ctx.key_id.unwrap().to_string();
        let state = replica(db.clone(), Arc::new(RateLimiter::new())).await;
        let none = RequestAttribution::default();

        // The key is reserved first, then the app. Drive `begin()` one poll
        // at a time, and between polls wait, without polling, for the app's
        // reservation to land: once it has, `begin()` is waiting on that
        // answer, with the key's reservation taken and counted as charged.
        // Once the key's has landed, the wait is long, so no poll can come
        // after the app's answer and let `begin()` finish.
        let mut begun = Box::pin(begin(&state, Some(&ctx), &none, "mock/image-gen", 0.5));
        loop {
            assert!(futures::poll!(begun.as_mut()).is_pending(), "begin() finished before it could be cut off");
            let key_reserved = spend_of(&db, EntityType::Key, &key).await > 0.0;
            let deadline = Instant::now() + Duration::from_millis(if key_reserved { 2000 } else { 20 });
            while spend_of(&db, EntityType::App, &app).await == 0.0 && Instant::now() < deadline {
                tokio::time::sleep(Duration::from_millis(2)).await;
            }
            if spend_of(&db, EntityType::App, &app).await > 0.0 {
                break;
            }
        }
        assert_eq!(spend_of(&db, EntityType::Key, &key).await, 0.5, "the key's estimate is reserved");
        drop(begun); // cut off inside the loop

        // The release is a task the drop spawns. (The app's reservation
        // landed, but its answer never reached `begin()`: that one stays.)
        let deadline = Instant::now() + Duration::from_secs(2);
        let mut left = spend_of(&db, EntityType::Key, &key).await;
        while left != 0.0 && Instant::now() < deadline {
            tokio::time::sleep(Duration::from_millis(20)).await;
            left = spend_of(&db, EntityType::Key, &key).await;
        }
        assert_eq!(left, 0.0, "a request cut off inside the reserve loop left {left} USD reserved on the key");
    }

    /// A settled guard's charge stands when it is dropped: the drop releases
    /// only what was never settled.
    #[tokio::test]
    async fn a_settled_guard_keeps_its_charge_when_dropped() {
        let db = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let ctx = capped_key(&db).await;
        let state = replica(db.clone(), Arc::new(RateLimiter::new())).await;
        let mut guard = begin(&state, Some(&ctx), &RequestAttribution::default(), "mock/image-gen", 0.5).await.unwrap();
        guard.settle(&state, 0.3).await;
        drop(guard);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert!((key_spend(&db, &ctx).await - 0.3).abs() < 1e-9, "the settled charge stands");
    }

    #[tokio::test]
    async fn begin_falls_back_to_a_local_slot_when_redis_is_down() {
        let limiter = Arc::new(RateLimiter::with_shared(dead_redis(false).await));
        let db = Arc::new(crate::db::sqlite::SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let ctx = capped_key(&db).await;
        let state = replica(db, limiter).await;
        let none = RequestAttribution::default();
        let first = begin(&state, Some(&ctx), &none, "mock/image-gen", 0.0).await.unwrap();
        assert_eq!(in_flight(EntityType::Key, &ctx.key_id.unwrap().to_string()), 1);
        let refused = begin(&state, Some(&ctx), &none, "mock/image-gen", 0.0).await.unwrap_err();
        assert_eq!(error_code(refused).await, (StatusCode::TOO_MANY_REQUESTS, "parallel_limit_exceeded".to_string()));
        drop(first);
        assert_eq!(in_flight(EntityType::Key, &ctx.key_id.unwrap().to_string()), 0);
    }
}
