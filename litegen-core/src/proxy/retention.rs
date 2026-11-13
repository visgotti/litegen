//! Deleting kept outputs after their app's `retention_days`
//! (docs/storage-access.md).
//!
//! Every replica sweeps hourly. A sweep has a pass for each app with a
//! retention of its own, then one with the gateway's default for every other
//! app, and each deletes the files of finished generations older than that
//! from whichever store holds them. The passes take turns, a batch each, and a
//! pass goes on while it deletes (up to [`RETENTION_BUDGET`] in all). A row is
//! marked only once its files are gone. A delete that fails puts its row
//! behind the rows not yet tried: it is tried again at the end of its pass if
//! that pass is still deleting, or else by a later sweep. Two replicas
//! deleting the same file is harmless.

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
            let sweep = sweep_until_done(&presenting, chrono::Utc::now(), RETENTION_BATCH, RETENTION_BUDGET);
            let Some(report) = crate::supervise::guarded("a retention sweep", sweep).await else { continue };
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
}
