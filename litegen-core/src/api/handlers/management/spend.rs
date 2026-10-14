//! Spend reporting (LiteLLM `/spend/logs`, `/global/spend/report`): spend
//! grouped by any attribution dimension, and the attributed request log.

use std::sync::Arc;

use axum::extract::{Query, State};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::{Extension, Json};
use chrono::{DateTime, NaiveDate, Utc};
use serde::Deserialize;

use super::{bad_request, ctx_of, forbidden, internal, require_org, store, Access};
use crate::api::middleware::{is_platform_admin, AppState, KeyContext};
use crate::auth::permissions::Permission;
use crate::management::types::*;
use crate::types::ErrorResponse;

#[derive(Debug, Default, Deserialize, utoipa::IntoParams)]
pub struct SpendQuery {
    /// day | model | provider | media_type | status | org | app | key | user | end_user | tag (report only; default day)
    pub group_by: Option<String>,
    /// Inclusive lower bound: RFC 3339 timestamp or `YYYY-MM-DD`.
    pub from: Option<String>,
    /// Inclusive upper bound: RFC 3339 timestamp or `YYYY-MM-DD` (the whole day).
    pub to: Option<String>,
    /// Platform admin only: another org. Omit for the caller's org (or every org, for a hosted platform admin).
    pub org_id: Option<String>,
    pub app_id: Option<String>,
    pub key_id: Option<String>,
    pub user_id: Option<String>,
    pub end_user: Option<String>,
    pub model: Option<String>,
    pub provider: Option<String>,
    pub tag: Option<String>,
    /// Spend logs only (default 1).
    pub page: Option<u32>,
    /// Spend logs only (default 50, max 500).
    pub per_page: Option<u32>,
}

fn parse_bound(raw: Option<&str>, end_of_day: bool, name: &str) -> Result<Option<DateTime<Utc>>, Response> {
    let Some(s) = raw.map(str::trim).filter(|s| !s.is_empty()) else { return Ok(None) };
    if let Ok(dt) = DateTime::parse_from_rfc3339(s) {
        return Ok(Some(dt.with_timezone(&Utc)));
    }
    if let Ok(d) = NaiveDate::parse_from_str(s, "%Y-%m-%d") {
        let t = if end_of_day { d.and_hms_opt(23, 59, 59) } else { d.and_hms_opt(0, 0, 0) };
        return Ok(t.map(|t| t.and_utc()));
    }
    Err(bad_request("invalid_date", &format!("{name} must be an RFC 3339 timestamp or YYYY-MM-DD")))
}

/// Turn the query into a filter the caller is allowed to run.
async fn build_filter(state: &AppState, ctx: &KeyContext, q: &SpendQuery) -> Result<SpendFilter, Response> {
    let explicit = q.org_id.as_deref().filter(|s| !s.is_empty());
    let org_id = if is_platform_admin(ctx) {
        explicit.map(str::to_string).or_else(|| ctx.org_id.clone())
    } else {
        let org = super::active_org(ctx, explicit)?;
        require_org(state, ctx, &org, Access::Read, Permission::OrgRead).await?;
        Some(org)
    };
    // An app-bound API key only ever sees its own app.
    let mut app_id = q.app_id.clone().filter(|s| !s.is_empty());
    if ctx.user.is_none() && ctx.key_id.is_some() {
        if let Some(bound) = &ctx.app_id {
            if app_id.as_ref().is_some_and(|a| a != bound) {
                return Err(forbidden("this API key can only report on its own application"));
            }
            app_id = Some(bound.clone());
        }
    }
    let from = parse_bound(q.from.as_deref(), false, "from")?;
    let to = parse_bound(q.to.as_deref(), true, "to")?;
    if let (Some(f), Some(t)) = (from, to) {
        if f > t {
            return Err(bad_request("invalid_date", "from must not be after to"));
        }
    }
    let clean = |v: &Option<String>| v.clone().filter(|s| !s.is_empty());
    Ok(SpendFilter {
        org_id,
        app_id,
        from,
        to,
        key_id: clean(&q.key_id),
        user_id: clean(&q.user_id),
        end_user: clean(&q.end_user),
        model: clean(&q.model),
        provider: clean(&q.provider),
        tag: clean(&q.tag),
    })
}

/// GET /v1/spend/report — Spend and request counts grouped by one dimension.
#[utoipa::path(
    get,
    path = "/v1/spend/report",
    params(SpendQuery),
    responses(
        (status = 200, description = "Grouped spend", body = SpendReportResponse),
        (status = 400, description = "Bad group_by or date", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    ),
    tag = "Spend"
)]
pub async fn spend_report(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Query(q): Query<SpendQuery>) -> Response {
    let ctx = match ctx_of(ctx) { Ok(c) => c, Err(r) => return r };
    let store = match store(&state) { Ok(s) => s, Err(r) => return r };
    let group_by = match q.group_by.as_deref().filter(|s| !s.is_empty()) {
        None => SpendGroupBy::Day,
        Some(s) => match SpendGroupBy::parse(s) {
            Some(g) => g,
            None => {
                return bad_request(
                    "invalid_group_by",
                    "group_by must be day, model, provider, media_type, status, org, app, key, user, end_user or tag",
                )
            }
        },
    };
    let filter = match build_filter(&state, &ctx, &q).await { Ok(f) => f, Err(r) => return r };
    match store.spend_report(&filter, group_by).await {
        Ok(data) => {
            // Tag groups can count one request under several tags, so totals
            // come from the ungrouped dimension instead of summing rows.
            let (total_requests, total_cost_usd) = if group_by == SpendGroupBy::Tag {
                match store.spend_report(&filter, SpendGroupBy::Org).await {
                    Ok(rows) => rows.iter().fold((0, 0.0), |(n, c), r| (n + r.requests, c + r.cost_usd)),
                    Err(e) => return internal(&e.to_string()),
                }
            } else {
                data.iter().fold((0, 0.0), |(n, c), r| (n + r.requests, c + r.cost_usd))
            };
            (
                StatusCode::OK,
                Json(SpendReportResponse { group_by, from: filter.from, to: filter.to, total_requests, total_cost_usd, data }),
            )
                .into_response()
        }
        Err(e) => internal(&e.to_string()),
    }
}

/// GET /v1/spend/logs — Attributed request log (key, user, end user, tags), newest first.
#[utoipa::path(
    get,
    path = "/v1/spend/logs",
    params(SpendQuery),
    responses(
        (status = 200, description = "Spend log page", body = SpendLogsResponse),
        (status = 400, description = "Bad date", body = ErrorResponse),
        (status = 403, description = "Forbidden", body = ErrorResponse),
    ),
    tag = "Spend"
)]
pub async fn spend_logs(State(state): State<Arc<AppState>>, ctx: Option<Extension<KeyContext>>, Query(q): Query<SpendQuery>) -> Response {
    let ctx = match ctx_of(ctx) { Ok(c) => c, Err(r) => return r };
    let store = match store(&state) { Ok(s) => s, Err(r) => return r };
    let filter = match build_filter(&state, &ctx, &q).await { Ok(f) => f, Err(r) => return r };
    let page = q.page.unwrap_or(1).max(1);
    let per_page = q.per_page.unwrap_or(50).clamp(1, 500);
    match store.spend_logs(&filter, page, per_page).await {
        Ok((data, total)) => (StatusCode::OK, Json(SpendLogsResponse { data, total, page, per_page })).into_response(),
        Err(e) => internal(&e.to_string()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bounds_accept_dates_and_timestamps() {
        let from = parse_bound(Some("2026-10-01"), false, "from").unwrap().unwrap();
        assert_eq!(from.to_rfc3339(), "2026-10-01T00:00:00+00:00");
        let to = parse_bound(Some("2026-10-01"), true, "to").unwrap().unwrap();
        assert_eq!(to.to_rfc3339(), "2026-10-01T23:59:59+00:00");
        let ts = parse_bound(Some("2026-10-01T12:00:00+02:00"), false, "from").unwrap().unwrap();
        assert_eq!(ts.to_rfc3339(), "2026-10-01T10:00:00+00:00");
        assert!(parse_bound(None, false, "from").unwrap().is_none());
        assert!(parse_bound(Some(" "), false, "from").unwrap().is_none());
        assert!(parse_bound(Some("yesterday"), false, "from").is_err());
    }
}
