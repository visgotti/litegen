//! [`ReservationStore`] for the SQLite and Postgres backends (migration 030).
//!
//! SQLite takes a reservation in one statement, the insert of its record,
//! whose trigger (migration 030) adds the amount; it settles, gives back and
//! sweeps in one transaction each, its single writer making each whole. Its
//! times are its own `strftime('%Y-%m-%d %H:%M:%f', 'now', ...)` text,
//! written and compared only in that format (as the poll claims and the
//! webhook outbox are). Postgres takes each step in one statement, its
//! data-modifying CTEs seeing one snapshot, except the sweep: one
//! transaction whose give-back takes its rows in a fixed order. Times are
//! the database's clock (`now()`). A release never takes a key or a limits
//! row below zero.

use std::collections::BTreeMap;

use async_trait::async_trait;
use uuid::Uuid;

use super::reservations::{HeldKey, Hold, IfGone, ReservationStore};
use super::types::EntityType;
use crate::db::{PostgresDatabase, SqliteDatabase};

/// Where a reservation is held: `reservations.target`.
const ON_KEY: &str = "api_key";
const ON_LIMITS: &str = "limits";

/// SQLite's `now`, in the text every reservation time is written in.
const SQLITE_NOW: &str = "strftime('%Y-%m-%d %H:%M:%f', 'now')";

/// A date-time modifier of fractional seconds, e.g. `+120.000 seconds`.
fn sqlite_seconds(d: chrono::Duration) -> String {
    format!("{:+.3} seconds", d.num_milliseconds() as f64 / 1000.0)
}

fn pg_seconds(d: chrono::Duration) -> f64 {
    d.num_milliseconds() as f64 / 1000.0
}

/// A deleted record: what it held, and where.
#[derive(sqlx::FromRow)]
struct Gone {
    target: String,
    entity_type: Option<String>,
    entity_id: String,
    amount: f64,
}

/// What the deleted records `gone` held, summed by row, in the one order
/// every give-back takes its rows in: keys by id, then limits rows by type
/// and id (`api_key` sorts before `limits`). Two sweeps that take their rows
/// in the same order never each hold one the other waits for.
fn by_row(gone: &[Gone]) -> BTreeMap<(&str, Option<&str>, &str), f64> {
    let mut by_row: BTreeMap<(&str, Option<&str>, &str), f64> = BTreeMap::new();
    for g in gone {
        *by_row.entry((g.target.as_str(), g.entity_type.as_deref(), g.entity_id.as_str())).or_default() += g.amount;
    }
    by_row
}

/// Give back what the deleted records `gone` held, one row at a time in
/// [`by_row`]'s order, in SQLite transaction `tx`.
async fn give_back_sqlite(tx: &mut sqlx::SqliteConnection, gone: &[Gone]) -> Result<(), sqlx::Error> {
    for ((target, entity_type, entity_id), amount) in by_row(gone) {
        match target {
            ON_KEY => {
                sqlx::query("UPDATE api_keys SET tokens_used = MAX(tokens_used - ?, 0) WHERE id = ?")
                    .bind(amount)
                    .bind(entity_id)
                    .execute(&mut *tx)
                    .await?;
            }
            ON_LIMITS => {
                sqlx::query("UPDATE entity_limits SET spend_usd = MAX(spend_usd - ?, 0) WHERE entity_type = ? AND entity_id = ?")
                    .bind(amount)
                    .bind(entity_type)
                    .bind(entity_id)
                    .execute(&mut *tx)
                    .await?;
            }
            other => tracing::warn!(held_on = other, entity = entity_id, "a reservation record names a target litegen doesn't know; its record is deleted, nothing given back"),
        }
    }
    Ok(())
}

/// Give back what the deleted records `gone` held, one row at a time in
/// [`by_row`]'s order, in Postgres transaction `tx`: each UPDATE takes its
/// row's lock, so the locks are taken in that order too.
async fn give_back_pg(tx: &mut sqlx::PgConnection, gone: &[Gone]) -> Result<(), sqlx::Error> {
    for ((target, entity_type, entity_id), amount) in by_row(gone) {
        match target {
            ON_KEY => {
                sqlx::query("UPDATE api_keys SET tokens_used = GREATEST(tokens_used - $1, 0) WHERE id = $2")
                    .bind(amount)
                    .bind(entity_id)
                    .execute(&mut *tx)
                    .await?;
            }
            ON_LIMITS => {
                sqlx::query("UPDATE entity_limits SET spend_usd = GREATEST(spend_usd - $1, 0) WHERE entity_type = $2 AND entity_id = $3")
                    .bind(amount)
                    .bind(entity_type)
                    .bind(entity_id)
                    .execute(&mut *tx)
                    .await?;
            }
            other => tracing::warn!(held_on = other, entity = entity_id, "a reservation record names a target litegen doesn't know; its record is deleted, nothing given back"),
        }
    }
    Ok(())
}

#[async_trait]
impl ReservationStore for SqliteDatabase {
    async fn reserve_key_quota(&self, hold: &Hold, key_id: &Uuid, amount: f64) -> Result<bool, sqlx::Error> {
        // One statement: the record is selected only while the amount fits
        // under the key's quota, and the trigger `reservations_take`
        // (migration 030) adds the amount to the key. Without the trigger
        // the record is written and nothing taken: `takes_reservations`
        // tells, and the upkeep logs it at startup.
        let taken = sqlx::query(&format!(
            "INSERT INTO reservations (id, holder, target, entity_type, entity_id, amount, created_at, expires_at) \
             SELECT ?1, ?2, ?3, NULL, id, ?4, {SQLITE_NOW}, strftime('%Y-%m-%d %H:%M:%f', 'now', ?5) \
             FROM api_keys WHERE id = ?6 AND (token_quota IS NULL OR tokens_used + ?4 <= token_quota)"
        ))
        .bind(&hold.id)
        .bind(&hold.holder)
        .bind(ON_KEY)
        .bind(amount)
        .bind(sqlite_seconds(hold.lease))
        .bind(key_id.to_string())
        .execute(self.pool())
        .await?;
        Ok(taken.rows_affected() == 1)
    }

    async fn reserve_limits(&self, hold: &Hold, entity_type: EntityType, entity_id: &str, amount: f64) -> Result<bool, sqlx::Error> {
        // As for a key: one statement, under the row's hard cap, the
        // trigger `reservations_take` adding the amount to the row.
        let taken = sqlx::query(&format!(
            "INSERT INTO reservations (id, holder, target, entity_type, entity_id, amount, created_at, expires_at) \
             SELECT ?1, ?2, ?3, entity_type, entity_id, ?4, {SQLITE_NOW}, strftime('%Y-%m-%d %H:%M:%f', 'now', ?5) \
             FROM entity_limits WHERE entity_type = ?6 AND entity_id = ?7 AND (max_budget_usd IS NULL OR spend_usd + ?4 <= max_budget_usd)"
        ))
        .bind(&hold.id)
        .bind(&hold.holder)
        .bind(ON_LIMITS)
        .bind(amount)
        .bind(sqlite_seconds(hold.lease))
        .bind(entity_type.as_str())
        .bind(entity_id)
        .execute(self.pool())
        .await?;
        Ok(taken.rows_affected() == 1)
    }

    async fn settle_key_quota(&self, id: &str, key_id: &Uuid, actual: f64, if_gone: IfGone) -> Result<bool, sqlx::Error> {
        let key = key_id.to_string();
        let mut tx = self.pool().begin().await?;
        let held: Option<f64> = sqlx::query_scalar("DELETE FROM reservations WHERE id = ? AND target = ? AND entity_id = ? RETURNING amount")
            .bind(id)
            .bind(ON_KEY)
            .bind(&key)
            .fetch_optional(&mut *tx)
            .await?;
        if held.is_none() && if_gone == IfGone::DoNothing {
            return Ok(false);
        }
        let delta = actual - held.unwrap_or(0.0);
        if delta.abs() > f64::EPSILON {
            sqlx::query("UPDATE api_keys SET tokens_used = MAX(tokens_used + ?, 0) WHERE id = ?")
                .bind(delta)
                .bind(&key)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;
        Ok(held.is_some())
    }

    async fn settle_limits(&self, id: &str, entity_type: EntityType, entity_id: &str, actual: f64, if_gone: IfGone) -> Result<Option<f64>, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let held: Option<f64> = sqlx::query_scalar(
            "DELETE FROM reservations WHERE id = ? AND target = ? AND entity_type = ? AND entity_id = ? RETURNING amount",
        )
        .bind(id)
        .bind(ON_LIMITS)
        .bind(entity_type.as_str())
        .bind(entity_id)
        .fetch_optional(&mut *tx)
        .await?;
        if held.is_none() && if_gone == IfGone::DoNothing {
            return Ok(None);
        }
        let spend: Option<f64> = sqlx::query_scalar(
            "UPDATE entity_limits SET spend_usd = MAX(spend_usd + ?, 0) WHERE entity_type = ? AND entity_id = ? RETURNING spend_usd",
        )
        .bind(actual - held.unwrap_or(0.0))
        .bind(entity_type.as_str())
        .bind(entity_id)
        .fetch_optional(&mut *tx)
        .await?;
        tx.commit().await?;
        Ok(spend)
    }

    async fn release(&self, id: &str) -> Result<bool, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let gone: Option<Gone> = sqlx::query_as("DELETE FROM reservations WHERE id = ? RETURNING target, entity_type, entity_id, amount")
            .bind(id)
            .fetch_optional(&mut *tx)
            .await?;
        if let Some(g) = &gone {
            give_back_sqlite(&mut tx, std::slice::from_ref(g)).await?;
        }
        tx.commit().await?;
        Ok(gone.is_some())
    }

    async fn renew(&self, holder: &str, ids: &[String], lease: chrono::Duration) -> Result<Vec<String>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_scalar(
            "UPDATE reservations SET expires_at = strftime('%Y-%m-%d %H:%M:%f', 'now', ?1) \
             WHERE holder = ?2 AND id IN (SELECT value FROM json_each(?3)) RETURNING id",
        )
        .bind(sqlite_seconds(lease))
        .bind(holder)
        .bind(serde_json::Value::from(ids.to_vec()).to_string())
        .fetch_all(self.pool())
        .await
    }

    async fn renew_idempotency(&self, keys: &[HeldKey], lease: chrono::Duration) -> Result<u64, sqlx::Error> {
        if keys.is_empty() {
            return Ok(0);
        }
        let held: Vec<serde_json::Value> = keys.iter().map(|k| serde_json::json!([k.scope, k.key, k.held_by])).collect();
        let renewed = sqlx::query(
            "UPDATE idempotency_keys SET held_until = strftime('%Y-%m-%d %H:%M:%f', 'now', ?1) \
             WHERE state = 'in_progress' AND (scope, idem_key, held_by) IN ( \
                 SELECT json_extract(value, '$[0]'), json_extract(value, '$[1]'), json_extract(value, '$[2]') FROM json_each(?2))",
        )
        .bind(sqlite_seconds(lease))
        .bind(serde_json::Value::from(held).to_string())
        .execute(self.pool())
        .await?;
        Ok(renewed.rows_affected())
    }

    async fn sweep(&self, limit: u32) -> Result<u64, sqlx::Error> {
        let mut tx = self.pool().begin().await?;
        let gone: Vec<Gone> = sqlx::query_as(&format!(
            "DELETE FROM reservations WHERE id IN ( \
                 SELECT id FROM reservations WHERE expires_at < {SQLITE_NOW} ORDER BY expires_at LIMIT ?) \
             RETURNING target, entity_type, entity_id, amount"
        ))
        .bind(limit as i64)
        .fetch_all(&mut *tx)
        .await?;
        give_back_sqlite(&mut tx, &gone).await?;
        tx.commit().await?;
        Ok(gone.len() as u64)
    }

    async fn takes_reservations(&self) -> Result<bool, sqlx::Error> {
        let triggers: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM sqlite_master WHERE type = 'trigger' AND name = 'reservations_take'")
            .fetch_one(self.pool())
            .await?;
        Ok(triggers > 0)
    }
}

#[async_trait]
impl ReservationStore for PostgresDatabase {
    async fn reserve_key_quota(&self, hold: &Hold, key_id: &Uuid, amount: f64) -> Result<bool, sqlx::Error> {
        let taken = sqlx::query(
            "WITH taken AS ( \
                 UPDATE api_keys SET tokens_used = tokens_used + $1 \
                 WHERE id = $2 AND (token_quota IS NULL OR tokens_used + $1 <= token_quota) \
                 RETURNING id \
             ) \
             INSERT INTO reservations (id, holder, target, entity_type, entity_id, amount, created_at, expires_at) \
             SELECT $3, $4, $5, NULL, taken.id, $1, now(), now() + make_interval(secs => $6) FROM taken",
        )
        .bind(amount)
        .bind(key_id.to_string())
        .bind(&hold.id)
        .bind(&hold.holder)
        .bind(ON_KEY)
        .bind(pg_seconds(hold.lease))
        .execute(self.pool())
        .await?;
        Ok(taken.rows_affected() == 1)
    }

    async fn reserve_limits(&self, hold: &Hold, entity_type: EntityType, entity_id: &str, amount: f64) -> Result<bool, sqlx::Error> {
        let taken = sqlx::query(
            "WITH taken AS ( \
                 UPDATE entity_limits SET spend_usd = spend_usd + $1 \
                 WHERE entity_type = $2 AND entity_id = $3 AND (max_budget_usd IS NULL OR spend_usd + $1 <= max_budget_usd) \
                 RETURNING entity_type, entity_id \
             ) \
             INSERT INTO reservations (id, holder, target, entity_type, entity_id, amount, created_at, expires_at) \
             SELECT $4, $5, $6, taken.entity_type, taken.entity_id, $1, now(), now() + make_interval(secs => $7) FROM taken",
        )
        .bind(amount)
        .bind(entity_type.as_str())
        .bind(entity_id)
        .bind(&hold.id)
        .bind(&hold.holder)
        .bind(ON_LIMITS)
        .bind(pg_seconds(hold.lease))
        .execute(self.pool())
        .await?;
        Ok(taken.rows_affected() == 1)
    }

    async fn settle_key_quota(&self, id: &str, key_id: &Uuid, actual: f64, if_gone: IfGone) -> Result<bool, sqlx::Error> {
        // `$5`: charge the whole actual when the record is gone.
        let held: i64 = sqlx::query_scalar(
            "WITH gone AS ( \
                 DELETE FROM reservations WHERE id = $1 AND target = $2 AND entity_id = $3 RETURNING amount \
             ), charged AS ( \
                 UPDATE api_keys SET tokens_used = GREATEST(tokens_used + $4 - COALESCE((SELECT amount FROM gone), 0), 0) \
                 WHERE id = $3 AND ($5 OR EXISTS (SELECT 1 FROM gone)) \
                 RETURNING 1 \
             ) \
             SELECT COUNT(*) FROM gone",
        )
        .bind(id)
        .bind(ON_KEY)
        .bind(key_id.to_string())
        .bind(actual)
        .bind(if_gone == IfGone::ChargeActual)
        .fetch_one(self.pool())
        .await?;
        Ok(held > 0)
    }

    async fn settle_limits(&self, id: &str, entity_type: EntityType, entity_id: &str, actual: f64, if_gone: IfGone) -> Result<Option<f64>, sqlx::Error> {
        // `$6`: charge the whole actual when the record is gone.
        sqlx::query_scalar(
            "WITH gone AS ( \
                 DELETE FROM reservations WHERE id = $1 AND target = $2 AND entity_type = $3 AND entity_id = $4 RETURNING amount \
             ) \
             UPDATE entity_limits SET spend_usd = GREATEST(spend_usd + $5 - COALESCE((SELECT amount FROM gone), 0), 0) \
             WHERE entity_type = $3 AND entity_id = $4 AND ($6 OR EXISTS (SELECT 1 FROM gone)) \
             RETURNING spend_usd",
        )
        .bind(id)
        .bind(ON_LIMITS)
        .bind(entity_type.as_str())
        .bind(entity_id)
        .bind(actual)
        .bind(if_gone == IfGone::ChargeActual)
        .fetch_optional(self.pool())
        .await
    }

    async fn release(&self, id: &str) -> Result<bool, sqlx::Error> {
        let gone: i64 = sqlx::query_scalar(&format!(
            "WITH gone AS ( \
                 DELETE FROM reservations WHERE id = $1 RETURNING target, entity_type, entity_id, amount \
             ), {GIVE_BACK} \
             SELECT COUNT(*) FROM gone"
        ))
        .bind(id)
        .fetch_one(self.pool())
        .await?;
        Ok(gone > 0)
    }

    async fn renew(&self, holder: &str, ids: &[String], lease: chrono::Duration) -> Result<Vec<String>, sqlx::Error> {
        if ids.is_empty() {
            return Ok(Vec::new());
        }
        sqlx::query_scalar("UPDATE reservations SET expires_at = now() + make_interval(secs => $1) WHERE holder = $2 AND id = ANY($3) RETURNING id")
            .bind(pg_seconds(lease))
            .bind(holder)
            .bind(ids.to_vec())
            .fetch_all(self.pool())
            .await
    }

    async fn renew_idempotency(&self, keys: &[HeldKey], lease: chrono::Duration) -> Result<u64, sqlx::Error> {
        if keys.is_empty() {
            return Ok(0);
        }
        let renewed = sqlx::query(
            "UPDATE idempotency_keys k SET held_until = now() + make_interval(secs => $1) \
             FROM unnest($2::text[], $3::text[], $4::text[]) AS h(scope, idem_key, held_by) \
             WHERE k.scope = h.scope AND k.idem_key = h.idem_key AND k.held_by = h.held_by AND k.state = 'in_progress'",
        )
        .bind(pg_seconds(lease))
        .bind(keys.iter().map(|k| k.scope.clone()).collect::<Vec<_>>())
        .bind(keys.iter().map(|k| k.key.clone()).collect::<Vec<_>>())
        .bind(keys.iter().map(|k| k.held_by.clone()).collect::<Vec<_>>())
        .execute(self.pool())
        .await?;
        Ok(renewed.rows_affected())
    }

    async fn sweep(&self, limit: u32) -> Result<u64, sqlx::Error> {
        // One transaction. The lapsed records no other sweep holds are
        // deleted (SKIP LOCKED), then what they held is given back one row at
        // a time in a fixed order (`by_row`): two sweeps never each hold a row
        // the other waits for, which one statement updating many rows in no
        // set order could (review round 3, finding 5).
        let mut tx = self.pool().begin().await?;
        let gone: Vec<Gone> = sqlx::query_as(
            "WITH lapsed AS MATERIALIZED ( \
                 SELECT id FROM reservations WHERE expires_at < now() ORDER BY expires_at LIMIT $1 FOR UPDATE SKIP LOCKED \
             ) \
             DELETE FROM reservations r USING lapsed WHERE r.id = lapsed.id \
             RETURNING r.target, r.entity_type, r.entity_id, r.amount",
        )
        .bind(i64::from(limit))
        .fetch_all(&mut *tx)
        .await?;
        give_back_pg(&mut tx, &gone).await?;
        tx.commit().await?;
        Ok(gone.len() as u64)
    }

    async fn takes_reservations(&self) -> Result<bool, sqlx::Error> {
        // One statement per reservation, no trigger (see the top of this file).
        Ok(true)
    }
}

/// The Postgres CTEs that give back what the deleted record of `gone` held:
/// a release's, one record and so one row (a sweep's many go row by row,
/// `give_back_pg`).
const GIVE_BACK: &str = "key_rows AS ( \
         UPDATE api_keys k SET tokens_used = GREATEST(k.tokens_used - s.amount, 0) \
         FROM (SELECT entity_id, SUM(amount) AS amount FROM gone WHERE target = 'api_key' GROUP BY entity_id) s \
         WHERE k.id = s.entity_id \
         RETURNING 1 \
     ), limit_rows AS ( \
         UPDATE entity_limits l SET spend_usd = GREATEST(l.spend_usd - s.amount, 0) \
         FROM (SELECT entity_type, entity_id, SUM(amount) AS amount FROM gone WHERE target = 'limits' GROUP BY entity_type, entity_id) s \
         WHERE l.entity_type = s.entity_type AND l.entity_id = s.entity_id \
         RETURNING 1 \
     )";
