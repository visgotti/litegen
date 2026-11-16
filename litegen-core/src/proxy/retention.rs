//! Deleting kept outputs after their app's `retention_days`
//! (docs/storage-access.md).
//!
//! Every replica sweeps hourly. A sweep has a pass for each app with a
//! retention of its own, then one with the gateway's default for every other
//! app, and each deletes the files of finished generations older than that
//! from whichever store holds them, and the request log's copy of the output
//! (the artifact's `output_value`; its prompt and params stay). The passes
//! take turns, a batch each, and a pass goes on while it deletes (up to
//! [`RETENTION_BUDGET`] in all). A row is marked only once its files and that
//! copy are gone. A delete that fails puts its row behind the rows not yet
//! tried: it is tried again at the end of its pass if that pass is still
//! deleting, or else by a later sweep. Two replicas deleting the same file is
//! harmless. Ages are judged on the database's clock, never a replica's own.
//!
//! Each round also deletes the webhook log past `webhooks.log_retention_days`
//! (0 unless set, which keeps it): outbox deliveries that ended, and attempt
//! rows made, longer ago than that. A pending delivery is never deleted.
//!
//! And it deletes the request log past `logs` ([`purge_request_log`]; both 0
//! unless set, which keeps it): artifacts after `logs.artifact_retention_days`
//! (or else `logs.retention_days`), and log rows, with their spend tags,
//! after `logs.retention_days`. Generation rows are never its to delete.

use std::sync::Arc;

use tracing::{info, warn};

use crate::access::present::Presenting;
use crate::proxy::storage::ImageStorage;
use crate::types::Generation;

/// The most generations one batch of a pass takes: the rows not tried yet,
/// oldest first, then those whose delete failed, least recently tried first.
pub const RETENTION_BATCH: usize = 500;

/// How long one sweep may go on taking batches while its passes delete (its
/// first round, a batch for every pass, always runs in full).
pub const RETENTION_BUDGET: std::time::Duration = std::time::Duration::from_secs(600);

/// What a sweep, or one batch of it, did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SweepReport {
    /// Generations whose files are now deleted (and their rows marked).
    pub generations: usize,
    /// Files deleted.
    pub files: usize,
    /// Tries that left a generation's files not all deleted, or its row not
    /// marked; the row stays due. A row tried again at the end of its pass
    /// counts again.
    pub failed: usize,
}

impl std::ops::AddAssign for SweepReport {
    fn add_assign(&mut self, batch: Self) {
        self.generations += batch.generations;
        self.files += batch.files;
        self.failed += batch.failed;
    }
}

/// The stores a generation's files can be in, and whether one is missing.
#[derive(Clone)]
pub struct FileStores {
    /// The app's own bucket first, when it can be used, then litegen's.
    pub stores: Vec<Arc<dyn ImageStorage>>,
    /// Why the app's own bucket can't be used right now: its settings can't
    /// be read, a role can't be assumed, or its address check fails. A kept
    /// file no store recognises may then still be in that bucket.
    pub unavailable: Option<String>,
}

/// The stores a file of `app_id` can be in: its own bucket first, then litegen's.
pub async fn stores_for(presenting: &Presenting, app_id: Option<&str>) -> FileStores {
    let mut found = FileStores { stores: vec![presenting.router.model3d_store.clone()], unavailable: None };
    if let Some(app) = app_id {
        match crate::api::handlers::app_store_for_files(&presenting.db, presenting.secrets_key, &presenting.router.config.aws, presenting.tenant_bucket(), app).await {
            Ok(Some(store)) => found.stores.insert(0, store),
            Ok(None) => {}
            Err(why) => found.unavailable = Some(why),
        }
    }
    found
}

/// Delete every file litegen kept for `gen` from whichever of `stores` holds
/// it. A vendor's own file (`kept: false`) and a URL no store recognises are
/// left alone, unless the app's own bucket can't be used right now: such a
/// file may still be in it, so nothing is called deleted. Returns how many
/// files were deleted.
pub async fn delete_generation_files(stores: &FileStores, gen: &Generation) -> Result<usize, String> {
    let mut deleted = 0;
    for asset in crate::api::handlers::generation_assets(gen) {
        if !asset.kept {
            continue;
        }
        let Some((store, key)) = stores.stores.iter().find_map(|s| s.key_of(&asset.url).map(|k| (s, k))) else {
            if let Some(why) = &stores.unavailable {
                return Err(format!("a file may still be in the app's bucket, which can't be used now: {why}"));
            }
            continue;
        };
        store.delete(&key).await.map_err(|e| format!("could not delete {key}: {e}"))?;
        deleted += 1;
    }
    Ok(deleted)
}

/// A sweep's passes, in the order they take turns: each app with a retention
/// of its own, then the gateway's retention for every other app (`None`). A
/// retention of 0 keeps files, so it has no pass. `None` when the apps can't
/// be listed.
async fn retention_passes(presenting: &Presenting) -> Option<Vec<(Option<String>, u32)>> {
    let config = &presenting.router.config.storage_access;
    let gateway = presenting.router.overrides().gateway;
    let mut passes: Vec<(Option<String>, u32)> = match presenting.db.apps_with_retention().await {
        Ok(apps) => apps
            .into_iter()
            .filter(|(_, days)| crate::access::retention_in_range(*days))
            .map(|(app, days)| (Some(app), days as u32))
            .collect(),
        Err(e) => {
            warn!(error = %e, "retention sweep: could not list apps with a retention of their own");
            return None;
        }
    };
    // Every app without a retention of its own follows the gateway's.
    passes.push((None, crate::access::effective(config, &gateway, None).retention_days));
    passes.retain(|(_, days)| *days != 0);
    Some(passes)
}

/// One batch of one pass, as of `now`: at most `limit` of the generations due
/// under a retention of `days`, `app_id`'s, or with `None`, those of every app
/// without a retention of its own.
async fn sweep_pass(
    presenting: &Presenting,
    app_id: Option<&str>,
    days: u32,
    now: chrono::DateTime<chrono::Utc>,
    limit: usize,
) -> SweepReport {
    let mut report = SweepReport::default();
    let cutoff = now - chrono::Duration::days(i64::from(days));
    let due = match presenting.db.generations_due_for_retention(app_id, cutoff, limit as i64).await {
        Ok(rows) => rows,
        Err(e) => {
            warn!(app_id = ?app_id, error = %e, "retention sweep: could not list due generations");
            return report;
        }
    };
    // Each app's bucket is resolved once per batch: a bucket that can't be
    // used would otherwise cost a lookup (and an STS call) per row.
    let mut by_app: std::collections::HashMap<Option<String>, FileStores> = std::collections::HashMap::new();
    for gen in due {
        let stores = match by_app.get(&gen.app_id) {
            Some(found) => found.clone(),
            None => {
                let found = stores_for(presenting, gen.app_id.as_deref()).await;
                by_app.insert(gen.app_id.clone(), found.clone());
                found
            }
        };
        match delete_generation_files(&stores, &gen).await {
            Ok(n) => {
                // The request log's copy of the output goes with the files
                // (an image's artifact keeps up to 2 MiB of it inline); the
                // prompt and params stay. Before the row is marked: a marked
                // row is never due again, so a clear that fails leaves it due
                // and a later try clears it.
                if let Err(e) = presenting.db.clear_artifact_output(&gen.id).await {
                    warn!(generation_id = %gen.id, error = %e, "retention sweep: files deleted, but the request log's copy of the output could not be cleared; it is tried again after the rows not yet tried");
                    report.failed += 1;
                    if let Err(e) = presenting.db.note_files_delete_failed(&gen.id).await {
                        warn!(generation_id = %gen.id, error = %e, "retention sweep: could not record the failed try");
                    }
                    continue;
                }
                if let Err(e) = presenting.db.mark_files_deleted(&gen.id, chrono::Utc::now(), "retention").await {
                    warn!(generation_id = %gen.id, error = %e, "retention sweep: files deleted, but the row could not be marked");
                    report.failed += 1;
                    continue;
                }
                report.generations += 1;
                report.files += n;
            }
            Err(e) => {
                warn!(generation_id = %gen.id, error = %e, "retention sweep: a file could not be deleted; it is tried again after the rows not yet tried");
                report.failed += 1;
                // Behind the rows not yet tried from now on: rows whose
                // deletes keep failing can't fill every batch.
                if let Err(e) = presenting.db.note_files_delete_failed(&gen.id).await {
                    warn!(generation_id = %gen.id, error = %e, "retention sweep: could not record the failed delete");
                }
            }
        }
    }
    report
}

/// One batch of a sweep, as of `now`: every pass once, at most `limit`
/// generations each.
pub async fn sweep_retention_once(presenting: &Presenting, now: chrono::DateTime<chrono::Utc>, limit: usize) -> SweepReport {
    let mut report = SweepReport::default();
    for (app_id, days) in retention_passes(presenting).await.unwrap_or_default() {
        report += sweep_pass(presenting, app_id.as_deref(), days, now, limit).await;
    }
    report
}

/// A whole sweep as of `now`. The passes take turns, a batch each (at most
/// `limit` generations), and a pass whose batch marks nothing (nothing due,
/// or only deletes that fail) sits out the rest of the sweep. The sweep ends
/// when no pass is left, or once `budget` is spent; its first round always
/// runs in full, so every pass gets a batch. A backlog bigger than one batch
/// is worked off in one sweep, not one batch an hour, and a pass whose bucket
/// refuses is tried once, not once per batch of the others.
pub async fn sweep_until_done(
    presenting: &Presenting,
    now: chrono::DateTime<chrono::Utc>,
    limit: usize,
    budget: std::time::Duration,
) -> SweepReport {
    let started = tokio::time::Instant::now();
    let mut total = SweepReport::default();
    // The passes whose last batch marked nothing.
    let mut finished: std::collections::HashSet<Option<String>> = std::collections::HashSet::new();
    let mut first_round = true;
    loop {
        // Read each round, so a retention changed during the sweep applies
        // from the next round.
        let Some(passes) = retention_passes(presenting).await else { return total };
        let mut deleting = false;
        for (app_id, days) in passes {
            if finished.contains(&app_id) {
                continue;
            }
            if !first_round && started.elapsed() >= budget {
                return total;
            }
            let batch = sweep_pass(presenting, app_id.as_deref(), days, now, limit).await;
            total += batch;
            if batch.generations == 0 {
                finished.insert(app_id);
            } else {
                deleting = true;
            }
        }
        if !deleting || started.elapsed() >= budget {
            return total;
        }
        first_round = false;
    }
}

/// The most rows of each table one batch of the webhook log's purge deletes.
pub const WEBHOOK_LOG_BATCH: u32 = 5000;

/// One hourly round, as of the database's clock: a whole sweep, a batch at
/// a time, then the purge of the webhook log ([`purge_webhook_log`]). A
/// cutoff is the database's time less a retention, never this replica's
/// own clock, which may run ahead (review 2026-11-11 R8). A round whose
/// database clock can't be read is skipped; the next one runs at its time.
pub async fn retention_round(presenting: &Presenting) -> SweepReport {
    let now = match presenting.db.clock_now().await {
        Ok(now) => now,
        Err(e) => {
            warn!(error = %e, "retention sweep: could not read the database's clock; the next round tries again");
            return SweepReport::default();
        }
    };
    let report = sweep_until_done(presenting, now, RETENTION_BATCH, RETENTION_BUDGET).await;
    let days = presenting.router.config.webhooks.log_retention_days;
    purge_webhook_log(&*presenting.db, days, WEBHOOK_LOG_BATCH, RETENTION_BUDGET).await;
    purge_request_log(&*presenting.db, &presenting.router.config.logs, now, REQUEST_LOG_BATCH, RETENTION_BUDGET).await;
    report
}

/// Delete the webhook log past `days` (`webhooks.log_retention_days`):
/// outbox deliveries that ended, and attempt rows made, more than `days`
/// days ago on the database's clock, never a pending delivery. A batch of
/// at most `batch` of each at a time, until a batch deletes fewer of both or
/// `budget` is spent, with [`SINGLE_WRITER_PAUSE`] between batches on
/// SQLite. 0 days keeps everything.
pub async fn purge_webhook_log(
    db: &dyn crate::db::DatabaseStore,
    days: u32,
    batch: u32,
    budget: std::time::Duration,
) -> crate::db::WebhookLogPurged {
    let mut total = crate::db::WebhookLogPurged::default();
    if days == 0 {
        return total;
    }
    // On SQLite, room for the writes the last batch held up.
    let pause = if db.single_writer() { SINGLE_WRITER_PAUSE } else { std::time::Duration::ZERO };
    let started = tokio::time::Instant::now();
    loop {
        match db.purge_webhook_log(days, batch).await {
            Ok(done) => {
                total.deliveries += done.deliveries;
                total.attempts += done.attempts;
                let full = u64::from(batch);
                if (done.deliveries < full && done.attempts < full) || started.elapsed() >= budget {
                    break;
                }
            }
            Err(e) => {
                warn!(error = %e, "retention sweep: could not delete the webhook log past its retention; the next round tries again");
                break;
            }
        }
        if !pause.is_zero() {
            tokio::time::sleep(pause).await;
            if started.elapsed() >= budget {
                break;
            }
        }
    }
    if total != crate::db::WebhookLogPurged::default() {
        info!(deliveries = total.deliveries, attempts = total.attempts, days, "retention sweep: deleted the webhook log past its retention");
    }
    total
}

/// The most rows one batch of the request log's purge deletes.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RequestLogBatch {
    /// Request log rows, each with its spend tags.
    pub logs: u32,
    /// Artifacts.
    pub artifacts: u32,
}

/// The request log's batches in the hourly round: 5000 log rows, as the
/// webhook log's, but 100 artifacts. An artifact can hold 2 MiB of output,
/// and on SQLite a batch holds the database's one writer while it runs, so
/// a batch of artifacts frees at most 200 MiB.
pub const REQUEST_LOG_BATCH: RequestLogBatch = RequestLogBatch { logs: 5000, artifacts: 100 };

/// How long the request log's and the webhook log's purges wait after a
/// batch that leaves more to do, on a database that takes one writer at a
/// time (SQLite). A write held
/// up by a batch waits at most 5 s (sqlx's busy timeout) and tries again at
/// most 100 ms apart, so it goes in during the pause; batches back to back
/// would hold every write for as long as a long purge runs.
pub const SINGLE_WRITER_PAUSE: std::time::Duration = std::time::Duration::from_millis(150);

/// What a purge of the request log deleted ([`purge_request_log`]).
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RequestLogPurged {
    /// Request log rows, each with its spend tags.
    pub logs: u64,
    /// Artifacts.
    pub artifacts: u64,
}

/// Delete the request log past `logs` (the `logs` config), as of `now` on
/// the database's clock: artifacts made more than `artifact_retention_days`
/// ago (or else `retention_days`), and log rows made more than
/// `retention_days` ago, each with its spend tags. An artifact can't outlive
/// its log row's retention, as config refuses a longer one. Never a
/// generation row: it has a lifecycle of its own. A batch of at most `batch`
/// of each at a time, until a batch deletes fewer of both or `budget` is
/// spent (the first batch always runs), with [`SINGLE_WRITER_PAUSE`] between
/// batches on SQLite. 0 days keeps everything, as before.
pub async fn purge_request_log(
    db: &dyn crate::db::DatabaseStore,
    logs: &crate::config::LogsConfig,
    now: chrono::DateTime<chrono::Utc>,
    batch: RequestLogBatch,
    budget: std::time::Duration,
) -> RequestLogPurged {
    let mut total = RequestLogPurged::default();
    let before = |days: u32| (days > 0).then(|| now - chrono::Duration::days(i64::from(days)));
    let logs_before = before(logs.retention_days);
    let artifacts_before = before(logs.artifact_retention_days).or(logs_before);
    if logs_before.is_none() && artifacts_before.is_none() {
        return total;
    }
    // A batch that deleted as many as it may leaves more due.
    let full = |deleted: u64, limit: u32| limit > 0 && deleted >= u64::from(limit);
    // On SQLite, room for the writes the last batch held up.
    let pause = if db.single_writer() { SINGLE_WRITER_PAUSE } else { std::time::Duration::ZERO };
    let started = tokio::time::Instant::now();
    loop {
        let mut more = false;
        if let Some(before) = artifacts_before {
            match db.purge_request_artifacts(before, batch.artifacts).await {
                Ok(n) => {
                    total.artifacts += n;
                    more |= full(n, batch.artifacts);
                }
                Err(e) => {
                    warn!(error = %e, "retention sweep: could not delete the request log's artifacts past their retention; the next round tries again");
                    break;
                }
            }
        }
        if let Some(before) = logs_before {
            match db.purge_request_logs(before, batch.logs).await {
                Ok(n) => {
                    total.logs += n;
                    more |= full(n, batch.logs);
                }
                Err(e) => {
                    warn!(error = %e, "retention sweep: could not delete the request log past its retention; the next round tries again");
                    break;
                }
            }
        }
        if !more || started.elapsed() >= budget {
            break;
        }
        if !pause.is_zero() {
            tokio::time::sleep(pause).await;
            if started.elapsed() >= budget {
                break;
            }
        }
    }
    if total != RequestLogPurged::default() {
        info!(
            logs = total.logs,
            artifacts = total.artifacts,
            retention_days = logs.retention_days,
            artifact_retention_days = logs.artifact_retention_days,
            "retention sweep: deleted the request log past its retention"
        );
    }
    total
}

/// Sweep every hour, the first time five minutes after start.
pub fn spawn_retention_sweep(presenting: Presenting) -> tokio::task::JoinHandle<()> {
    spawn_retention_sweep_every(presenting, std::time::Duration::from_secs(300), std::time::Duration::from_secs(3600))
}

/// Sweep every `every`, the first time `first` after start. A sweep that
/// panics (a store, a row) is logged, and the next one runs at its time.
pub fn spawn_retention_sweep_every(
    presenting: Presenting,
    first: std::time::Duration,
    every: std::time::Duration,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut tick = tokio::time::interval_at(tokio::time::Instant::now() + first, every);
        loop {
            tick.tick().await;
            let Some(report) = crate::supervise::guarded("a retention sweep", retention_round(&presenting)).await else { continue };
            if report != SweepReport::default() {
                info!(generations = report.generations, files = report.files, failed = report.failed, "retention sweep");
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicUsize, Ordering};

    use crate::config::AppConfig;
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::DatabaseStore;
    use crate::proxy::cache::GenerationCache;
    use crate::proxy::registry::ProviderRegistry;
    use crate::proxy::router::ProxyRouter;
    use crate::proxy::storage::{ImageStoreError, LocalModel3dStorage, LocalStore};

    /// litegen's local store, except that its first delete panics (a fixture
    /// for the sweep loop); every later call is the real store's.
    struct PanicsOnFirstDelete {
        inner: LocalModel3dStorage,
        deletes: Arc<AtomicUsize>,
    }

    #[async_trait::async_trait]
    impl ImageStorage for PanicsOnFirstDelete {
        async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, ImageStoreError> {
            self.inner.put(key, bytes, content_type).await
        }
        async fn delete(&self, key: &str) -> Result<(), ImageStoreError> {
            if self.deletes.fetch_add(1, Ordering::SeqCst) == 0 {
                panic!("a store that panics on its first delete");
            }
            self.inner.delete(key).await
        }
        async fn get(&self, key: &str) -> Result<Option<(bytes::Bytes, String)>, ImageStoreError> {
            self.inner.get(key).await
        }
        fn key_of(&self, url: &str) -> Option<String> {
            self.inner.key_of(url)
        }
    }

    /// A 3D generation that kept nothing (`keep_outputs: false`) has no file
    /// of litegen's: deleting its files looks for none in litegen's stores,
    /// so it can't be held up by an app bucket that can't be used now.
    #[tokio::test]
    async fn a_3d_generation_that_kept_nothing_has_no_file_of_litegens_to_delete() {
        let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let id = "litegen-3d-retention-kept-nothing";
        let url = "https://cdn.vendor.example/a/model.glb?sig=1";
        db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, None, None).await.unwrap();
        let meta = serde_json::json!({
            "keep_outputs": false,
            "assets": [{ "kind": "mesh", "format": "glb", "url": url, "kept": false }],
        });
        db.update_generation_if_active(id, "completed", 100, Some(url), None, Some(chrono::Utc::now()), Some(&meta)).await.unwrap();
        let gen = db.get_generation(id).await.unwrap().unwrap();
        let dir = tempfile::tempdir().unwrap();
        let stores = FileStores {
            stores: vec![Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()))],
            unavailable: Some("the app's bucket can't be used right now".into()),
        };
        assert_eq!(delete_generation_files(&stores, &gen).await, Ok(0), "the vendor's file is the vendor's to delete");
    }

    #[tokio::test]
    async fn a_sweep_that_panics_does_not_end_the_sweeps() {
        let concrete = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let db: Arc<dyn DatabaseStore> = concrete.clone();
        let mut config = AppConfig::default();
        config.storage_access.retention_days = 1;
        let cache = Arc::new(GenerationCache::new(&config.cache));
        let mut router = ProxyRouter::new(Arc::new(ProviderRegistry::new()), cache, Arc::new(config), Arc::new(LocalStore));
        let dir = tempfile::tempdir().unwrap();
        let deletes = Arc::new(AtomicUsize::new(0));
        router.model3d_store = Arc::new(PanicsOnFirstDelete {
            inner: LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()),
            deletes: deletes.clone(),
        });

        // A finished generation from three days ago, with one kept file.
        let id = "litegen-3d-retention-panic";
        let url = router
            .model3d_store
            .put(&format!("litegen/3d/{id}/model.glb"), &bytes::Bytes::from_static(b"glTF-ish"), "model/gltf-binary")
            .await
            .unwrap();
        db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, None, None).await.unwrap();
        let meta = serde_json::json!({ "assets": [{ "index": 0, "kind": "mesh", "format": "glb", "url": url }] });
        db.update_generation_if_active(id, "completed", 100, Some(&url), None, Some(chrono::Utc::now()), Some(&meta)).await.unwrap();
        sqlx::query("UPDATE generations SET created_at = datetime('now','-3 days') WHERE id = ?")
            .bind(id)
            .execute(concrete.pool())
            .await
            .unwrap();

        let presenting = Presenting { router: Arc::new(router), db: db.clone(), secrets_key: None, mode: crate::config::Mode::SingleTenant };
        let sweeps = spawn_retention_sweep_every(presenting, std::time::Duration::from_millis(10), std::time::Duration::from_millis(100));
        let mut marked = false;
        for _ in 0..60 {
            if db.get_generation(id).await.unwrap().unwrap().files_deleted_at.is_some() {
                marked = true;
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        let finished = sweeps.is_finished();
        sweeps.abort();
        assert!(marked, "a later sweep deletes the file and marks the row (deletes tried: {}, sweep task finished: {finished})", deletes.load(Ordering::SeqCst));
        assert!(deletes.load(Ordering::SeqCst) >= 2, "the first delete panicked and a later one ran");
    }

    /// A router over litegen's local file store in `dir`, keeping files
    /// `retention_days`, and the generation `id` it kept one file for.
    async fn a_kept_file(db: &Arc<dyn DatabaseStore>, dir: &std::path::Path, retention_days: u32, id: &str) -> ProxyRouter {
        let mut config = AppConfig::default();
        config.storage_access.retention_days = retention_days;
        let cache = Arc::new(GenerationCache::new(&config.cache));
        let mut router = ProxyRouter::new(Arc::new(ProviderRegistry::new()), cache, Arc::new(config), Arc::new(LocalStore));
        router.model3d_store = Arc::new(LocalModel3dStorage::new(dir, "http://127.0.0.1:4000".into()));
        let url = router
            .model3d_store
            .put(&format!("litegen/3d/{id}/model.glb"), &bytes::Bytes::from_static(b"glTF-ish"), "model/gltf-binary")
            .await
            .unwrap();
        db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, None, None).await.unwrap();
        let meta = serde_json::json!({ "assets": [{ "index": 0, "kind": "mesh", "format": "glb", "url": url }] });
        db.update_generation_if_active(id, "completed", 100, Some(&url), None, Some(chrono::Utc::now()), Some(&meta)).await.unwrap();
        router
    }

    /// The sweep judges a row's age on the database's clock, never on this
    /// replica's own (review 2026-11-11 R8): a replica whose clock runs 3
    /// days ahead must not delete files a 4-day retention still keeps (2
    /// days old on the database's clock, 5 on the replica's).
    #[tokio::test]
    async fn a_sweep_judges_age_on_the_databases_clock() {
        let skewed = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap().with_clock_offset(-chrono::Duration::days(3)));
        let db: Arc<dyn DatabaseStore> = skewed.clone();
        let dir = tempfile::tempdir().unwrap();
        let id = "litegen-3d-retention-skewed";
        let router = a_kept_file(&db, dir.path(), 4, id).await;
        sqlx::query("UPDATE generations SET created_at = datetime('now','-5 days') WHERE id = ?").bind(id).execute(skewed.pool()).await.unwrap();

        let presenting = Presenting { router: Arc::new(router), db: db.clone(), secrets_key: None, mode: crate::config::Mode::SingleTenant };
        let report = retention_round(&presenting).await;
        assert_eq!(report, SweepReport::default(), "nothing is due on the database's clock");
        assert!(db.get_generation(id).await.unwrap().unwrap().files_deleted_at.is_none(), "its file is kept");
    }

    /// Each round also purges the webhook log once `webhooks.log_retention_days`
    /// is set: deliveries that ended, and attempts made, longer ago than
    /// that. A pending delivery stays however old. Unset, or 0, everything is
    /// kept, as before the purge.
    #[tokio::test]
    async fn a_round_purges_the_webhook_log_past_its_retention() {
        for (days, ended_kept) in [(None, true), (Some(0), true), (Some(30), false)] {
            let concrete = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
            let db: Arc<dyn DatabaseStore> = concrete.clone();
            let key = db.create_api_key("k", "hash-k", "lg-k", None, None, "generate,read", Some("https://hooks.example/h")).await.unwrap();
            for id in ["g-ended", "g-pending"] {
                db.insert_generation(id, Some(&key.id), "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.unwrap();
                db.update_generation_if_active(id, "completed", 100, None, None, Some(chrono::Utc::now()), None).await.unwrap();
            }
            sqlx::query(
                "UPDATE webhook_outbox SET created_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-90 days'), \
                 status = CASE generation_id WHEN 'g-ended' THEN 'delivered' ELSE 'pending' END, \
                 finished_at = CASE generation_id WHEN 'g-ended' THEN strftime('%Y-%m-%d %H:%M:%f', 'now', '-31 days') END",
            )
            .execute(concrete.pool())
            .await
            .unwrap();
            let mut config = AppConfig::default();
            if let Some(days) = days {
                config.webhooks.log_retention_days = days;
            }
            let cache = Arc::new(GenerationCache::new(&config.cache));
            let router = ProxyRouter::new(Arc::new(ProviderRegistry::new()), cache, Arc::new(config), Arc::new(LocalStore));
            let presenting = Presenting { router: Arc::new(router), db: db.clone(), secrets_key: None, mode: crate::config::Mode::SingleTenant };

            retention_round(&presenting).await;
            assert_eq!(db.webhook_outbox_of("g-ended").await.unwrap().is_some(), ended_kept, "ended 31 days ago, retention {days:?}");
            assert!(db.webhook_outbox_of("g-pending").await.unwrap().is_some(), "a pending delivery stays, retention {days:?}");
        }
    }

    use crate::config::LogsConfig;

    /// What the request log still has of a request: its row, how many spend
    /// tags, its artifact.
    type LogParts = (bool, i64, bool);
    const WHOLE: LogParts = (true, 2, true);
    const ROW_AND_TAGS: LogParts = (true, 2, false);
    const GONE: LogParts = (false, 0, false);

    /// Request `id` in the request log, made `days` days ago on this
    /// process's clock: its row, two spend tags and its artifact.
    async fn a_logged_request(db: &SqliteDatabase, id: &str, days: i64) {
        db.log_request(id, "mock/image-gen", "mock", "completed", "image", 0.04, 5, None, None, None, None).await.unwrap();
        let tags = crate::management::types::Attribution { tags: vec!["campaign".into(), "client".into()], ..Default::default() };
        db.management().unwrap().attribute_request_log(id, &tags).await.unwrap();
        let artifact = crate::types::RequestArtifact {
            request_id: id.to_string(),
            media_type: "image".to_string(),
            prompt: Some("a red cube".to_string()),
            negative_prompt: None,
            params_json: None,
            refs_meta_json: None,
            output_kind: "url".to_string(),
            output_value: Some("https://cdn.example/a.png".to_string()),
            output_mime: Some("image/png".to_string()),
            output_truncated: false,
            error_message: None,
            created_at: chrono::Utc::now(),
            org_id: None,
            app_id: None,
        };
        db.insert_request_artifact(&artifact).await.unwrap();
        let ago = format!("-{days} days");
        for stmt in ["UPDATE request_logs SET created_at = datetime('now', ?) WHERE id = ?", "UPDATE request_artifacts SET created_at = datetime('now', ?) WHERE request_id = ?"] {
            sqlx::query(stmt).bind(&ago).bind(id).execute(db.pool()).await.unwrap();
        }
    }

    async fn what_is_left_of(db: &SqliteDatabase, id: &str) -> LogParts {
        let count = |sql: &'static str| sqlx::query_scalar::<_, i64>(sql).bind(id.to_string()).fetch_one(db.pool());
        (
            count("SELECT COUNT(*) FROM request_logs WHERE id = ?").await.unwrap() == 1,
            count("SELECT COUNT(*) FROM request_log_tags WHERE request_id = ?").await.unwrap(),
            count("SELECT COUNT(*) FROM request_artifacts WHERE request_id = ?").await.unwrap() == 1,
        )
    }

    /// A round's view of `db`, under the request log retention `logs`.
    fn presenting_with_logs(db: Arc<dyn DatabaseStore>, logs: LogsConfig) -> Presenting {
        let config = AppConfig { logs, ..AppConfig::default() };
        let cache = Arc::new(GenerationCache::new(&config.cache));
        let router = ProxyRouter::new(Arc::new(ProviderRegistry::new()), cache, Arc::new(config), Arc::new(LocalStore));
        Presenting { router: Arc::new(router), db, secrets_key: None, mode: crate::config::Mode::SingleTenant }
    }

    /// Each round purges the request log past `logs.retention_days`: a row
    /// made longer ago than that goes with its tags and its artifact, a newer
    /// one stays whole, and so does a generation that shares the old row's id
    /// (an async job's). Unset (0), everything stays, as before the purge.
    #[tokio::test]
    async fn a_round_purges_the_request_log_past_its_retention() {
        for (retention_days, old_left) in [(30, GONE), (0, WHOLE)] {
            let concrete = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
            let db: Arc<dyn DatabaseStore> = concrete.clone();
            a_logged_request(&concrete, "req-old", 31).await;
            a_logged_request(&concrete, "req-new", 29).await;
            db.insert_generation("req-old", None, "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.unwrap();
            let presenting = presenting_with_logs(db.clone(), LogsConfig { retention_days, artifact_retention_days: 0 });

            retention_round(&presenting).await;
            assert_eq!(what_is_left_of(&concrete, "req-old").await, old_left, "31 days old, retention {retention_days}");
            assert_eq!(what_is_left_of(&concrete, "req-new").await, WHOLE, "29 days old, retention {retention_days}");
            assert!(db.get_generation("req-old").await.unwrap().is_some(), "the generation stays, retention {retention_days}");
        }
    }

    /// `logs.artifact_retention_days` deletes artifacts sooner. Alone, only
    /// artifacts go: every row and tag stays. Under a longer `retention_days`,
    /// a request between the two keeps its row and tags, an older one goes
    /// whole, and a newer one stays whole.
    #[tokio::test]
    async fn artifacts_go_after_their_own_retention_and_their_rows_stay() {
        for (retention_days, left) in [(0, [ROW_AND_TAGS, ROW_AND_TAGS, WHOLE]), (30, [GONE, ROW_AND_TAGS, WHOLE])] {
            let concrete = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
            let db: Arc<dyn DatabaseStore> = concrete.clone();
            let requests = [("req-31-days", 31), ("req-10-days", 10), ("req-3-days", 3)];
            for (id, days) in requests {
                a_logged_request(&concrete, id, days).await;
            }
            let presenting = presenting_with_logs(db, LogsConfig { retention_days, artifact_retention_days: 7 });

            retention_round(&presenting).await;
            for ((id, _), want) in requests.into_iter().zip(left) {
                assert_eq!(what_is_left_of(&concrete, id).await, want, "{id}, retention {retention_days}, artifacts 7");
            }
        }
    }

    /// The purge goes a batch at a time while batches come back full, within
    /// its budget, as the webhook log's does: the first batch always runs,
    /// and no further one starts once the budget is spent.
    #[tokio::test]
    async fn the_request_log_purge_goes_a_batch_at_a_time_within_its_budget() {
        let concrete = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let db: Arc<dyn DatabaseStore> = concrete.clone();
        let ids = ["req-1", "req-2", "req-3"];
        for id in ids {
            a_logged_request(&concrete, id, 31).await;
        }
        let logs = LogsConfig { retention_days: 30, artifact_retention_days: 0 };
        let one = RequestLogBatch { logs: 1, artifacts: 1 };
        let now = db.clock_now().await.unwrap();

        let first = purge_request_log(&*db, &logs, now, one, std::time::Duration::ZERO).await;
        assert_eq!(first, RequestLogPurged { logs: 1, artifacts: 1 }, "one batch: the budget is spent");
        let rest = purge_request_log(&*db, &logs, now, one, std::time::Duration::from_secs(60)).await;
        assert_eq!(rest, RequestLogPurged { logs: 2, artifacts: 2 }, "the rest, a batch at a time");
        for id in ids {
            assert_eq!(what_is_left_of(&concrete, id).await, GONE, "{id}");
        }
    }

    /// SQLite takes one writer at a time, and a writer held up waits at most
    /// 5 s (sqlx's busy timeout), trying again at most 100 ms apart. Batches
    /// back to back would hold every request's write for as long as the
    /// purge runs, so after each batch that leaves more to do the purge waits
    /// [`SINGLE_WRITER_PAUSE`], longer than those 100 ms, and a write held up
    /// by a batch goes in before the next one.
    #[tokio::test]
    async fn on_sqlite_the_purge_lets_other_writes_in_between_batches() {
        let concrete = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let db: Arc<dyn DatabaseStore> = concrete.clone();
        for id in ["req-1", "req-2", "req-3"] {
            a_logged_request(&concrete, id, 31).await;
        }
        let logs = LogsConfig { retention_days: 30, artifact_retention_days: 0 };
        let now = db.clock_now().await.unwrap();

        let started = std::time::Instant::now();
        let purged = purge_request_log(&*db, &logs, now, RequestLogBatch { logs: 1, artifacts: 1 }, std::time::Duration::from_secs(60)).await;
        let took = started.elapsed();
        assert_eq!(purged, RequestLogPurged { logs: 3, artifacts: 3 });
        assert!(took >= 3 * SINGLE_WRITER_PAUSE, "a pause after each of the 3 full batches, none after the last: {took:?}");
    }

    /// The purge judges a request's age on the database's clock, as the
    /// sweep does: with that clock 3 days behind this replica's, a 4-day
    /// retention keeps a request made 5 days ago here (2 on the database's
    /// clock) and deletes one made 8 days ago here (5 there).
    #[tokio::test]
    async fn a_round_judges_the_request_logs_age_on_the_databases_clock() {
        let skewed = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap().with_clock_offset(-chrono::Duration::days(3)));
        let db: Arc<dyn DatabaseStore> = skewed.clone();
        a_logged_request(&skewed, "req-5-days", 5).await;
        a_logged_request(&skewed, "req-8-days", 8).await;
        let presenting = presenting_with_logs(db, LogsConfig { retention_days: 4, artifact_retention_days: 0 });

        retention_round(&presenting).await;
        assert_eq!(what_is_left_of(&skewed, "req-5-days").await, WHOLE, "2 days old on the database's clock");
        assert_eq!(what_is_left_of(&skewed, "req-8-days").await, GONE, "5 days old on the database's clock");
    }

    /// The webhook log's purge leaves SQLite's one writer free between its
    /// batches too, as the request log's does: back to back, a writer held
    /// up waits out the whole purge, past its 5 s busy timeout on a large
    /// log. After each batch that leaves more to do it waits
    /// [`SINGLE_WRITER_PAUSE`], so a write held up by a batch goes in before
    /// the next one.
    #[tokio::test]
    async fn on_sqlite_the_webhook_log_purge_lets_other_writes_in_between_batches() {
        let concrete = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let db: Arc<dyn DatabaseStore> = concrete.clone();
        let key = db.create_api_key("k", "hash-k", "lg-k", None, None, "generate,read", Some("https://hooks.example/h")).await.unwrap();
        db.insert_generation("g-hooked", Some(&key.id), "mock/video-gen", "mock", "video", Some("job"), 0.0, None, None).await.unwrap();
        for n in 1..=3 {
            let attempt = crate::types::WebhookDelivery {
                id: format!("wh-{n}"),
                key_id: key.id.to_string(),
                generation_id: "g-hooked".to_string(),
                url: "https://hooks.example/h".to_string(),
                attempt_number: n,
                status_code: Some(503),
                success: false,
                response_body: None,
                error_message: None,
                payload_json: String::new(),
                created_at: chrono::Utc::now(),
                delivery_id: None,
            };
            db.insert_webhook_delivery(&attempt).await.unwrap();
        }
        sqlx::query("UPDATE webhook_deliveries SET created_at = datetime('now', '-31 days')").execute(concrete.pool()).await.unwrap();

        let started = std::time::Instant::now();
        let purged = purge_webhook_log(&*db, 30, 1, std::time::Duration::from_secs(60)).await;
        let took = started.elapsed();
        assert_eq!(purged, crate::db::WebhookLogPurged { deliveries: 0, attempts: 3 });
        assert!(took >= 3 * SINGLE_WRITER_PAUSE, "a pause after each of the 3 full batches, none after the last: {took:?}");
    }
}
