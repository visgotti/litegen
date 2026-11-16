//! The files of deleted orgs and apps, deleted from litegen's own store
//! (migration 031; docs/storage-access.md, "Deleting an org or an app").
//!
//! Deleting an org or an app records each file litegen kept for its
//! generations (`tenant_file_deletions`) in the transaction that deletes
//! their rows, so a delete that commits can't lose its files, whatever then
//! happens to the process. The replica that deleted the org or app starts on
//! them at once ([`start_deleting`]), and every replica's worker
//! ([`spawn_tenant_file_deleter`]) takes the due ones every [`TICK`]: a pass
//! that was cut (a stop, a crash) or whose deletes failed is taken up again.
//!
//! A claim holds its rows for [`HOLD`] on the database's clock, and a batch
//! ([`BATCH`]) is small enough to end within it, so two workers never take
//! one row at once; a worker that dies leaves its rows due again once the
//! hold has passed. A file deleted is its row removed; a
//! delete that fails makes its row due again after [`retry_delay`], with the
//! error. Deleting a file that is gone already is no error (litegen's local
//! store and S3 both), so a file deleted twice does no harm.
//!
//! Only litegen's own store, as before: a URL it doesn't recognise (a file in
//! an app's own bucket, which is the customer's and stays; a vendor's link)
//! is let go without a delete.

use std::sync::Arc;

use tracing::{info, warn};

use crate::api::middleware::KeyContext;
use crate::db::{DatabaseStore, TenantFileDeletion};
use crate::proxy::storage::ImageStorage;

/// How long a claim holds the file deletions it took: past it (a worker that
/// died), another worker takes them.
pub const HOLD: std::time::Duration = std::time::Duration::from_secs(300);

/// How often each replica's worker looks for due file deletions.
pub const TICK: std::time::Duration = std::time::Duration::from_secs(60);

/// The most file deletions one claim takes: few enough that their deletes,
/// one after another and each allowed [`DELETE_TIMEOUT`], end within the
/// claim's [`HOLD`] with a minute to spare for the database's calls, so no
/// other worker takes a file of the batch while it runs. A pass claims batch
/// after batch, so this sets only how often it claims.
pub const BATCH: u32 = 4;

/// How long one delete may take before it counts as failed.
pub const DELETE_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(60);

const _: () = assert!(
    BATCH as u64 * DELETE_TIMEOUT.as_secs() + 60 <= HOLD.as_secs(),
    "a batch's deletes must end within its hold"
);

/// The wait before a file is tried again after its `attempts`-th failed
/// delete: a minute, doubling with each attempt, at most 6 hours.
pub fn retry_delay(attempts: i32) -> chrono::Duration {
    let doublings = attempts.saturating_sub(1).clamp(0, 16) as u32;
    chrono::Duration::minutes(1i64 << doublings).min(chrono::Duration::hours(6))
}

/// What a pass over the due file deletions did.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DeletionReport {
    /// Files deleted from litegen's store, their records removed.
    pub deleted: usize,
    /// URLs litegen's store doesn't recognise: nothing of litegen's to
    /// delete, their records removed.
    pub let_go: usize,
    /// Deletes that failed: their records are due again later.
    pub failed: usize,
}

impl std::ops::AddAssign for DeletionReport {
    fn add_assign(&mut self, batch: Self) {
        self.deleted += batch.deleted;
        self.let_go += batch.let_go;
        self.failed += batch.failed;
    }
}

/// One batch: claim up to `limit` due file deletions (with `of`, `(scope,
/// id)`, only that org's or app's) and delete each file from `store`,
/// litegen's own.
pub async fn delete_due(db: &dyn DatabaseStore, store: &dyn ImageStorage, of: Option<(&str, &str)>, limit: u32) -> DeletionReport {
    let mut report = DeletionReport::default();
    let hold = chrono::Duration::from_std(HOLD).unwrap_or_else(|_| chrono::Duration::minutes(5));
    let claimed = match db.claim_tenant_file_deletions(of, limit, hold).await {
        Ok(claimed) => claimed,
        Err(e) => {
            warn!(error = %e, "could not claim the due files of deleted orgs and apps; the next pass tries again");
            return report;
        }
    };
    for deletion in claimed {
        delete_one(db, store, &deletion, &mut report).await;
    }
    report
}

/// Delete the file of `d`, which this worker claimed, and settle its record.
async fn delete_one(db: &dyn DatabaseStore, store: &dyn ImageStorage, d: &TenantFileDeletion, report: &mut DeletionReport) {
    let Some(key) = store.key_of(&d.url) else {
        // Not in litegen's store: an app's own bucket (the customer's) or a
        // vendor's link. Nothing of litegen's to delete.
        match db.finish_tenant_file_deletion(d.id).await {
            Ok(_) => report.let_go += 1,
            Err(e) => {
                warn!(generation_id = %d.generation_id, error = %e, "could not remove the record of a file outside litegen's store; it is let go once its hold passes");
                report.failed += 1;
            }
        }
        return;
    };
    let deleted = match tokio::time::timeout(DELETE_TIMEOUT, store.delete(&key)).await {
        Ok(Ok(())) => Ok(()),
        Ok(Err(e)) => Err(e.to_string()),
        Err(_) => Err(format!("the delete took more than {} s", DELETE_TIMEOUT.as_secs())),
    };
    match deleted {
        Ok(()) => match db.finish_tenant_file_deletion(d.id).await {
            Ok(_) => report.deleted += 1,
            Err(e) => {
                // Taken again once its hold passes: deleting it again is no harm.
                warn!(generation_id = %d.generation_id, key = %key, error = %e, "a deleted org's or app's file is deleted, but its record could not be removed; it is tried again");
                report.failed += 1;
            }
        },
        Err(error) => {
            let after = retry_delay(d.attempts);
            warn!(
                scope = %d.scope, tenant_id = %d.tenant_id, generation_id = %d.generation_id, key = %key,
                attempts = d.attempts, retry_in_secs = after.num_seconds(), error = %error,
                "could not delete a deleted org's or app's file; it is tried again"
            );
            if let Err(e) = db.retry_tenant_file_deletion(d.id, d.attempts, after, &error).await {
                warn!(generation_id = %d.generation_id, error = %e, "could not record a failed delete; the file is tried again once its hold passes");
            }
            report.failed += 1;
        }
    }
}

/// Every due file deletion (with `of`, of that org or app), `batch` at a
/// time, while the batches delete: a batch that deletes nothing (the store
/// refuses every delete) ends it, and what is left waits for the next pass
/// rather than each being tried against a store that refuses now.
pub async fn delete_all_due(db: &dyn DatabaseStore, store: &dyn ImageStorage, of: Option<(&str, &str)>, batch: u32) -> DeletionReport {
    let mut total = DeletionReport::default();
    loop {
        let report = delete_due(db, store, of, batch).await;
        total += report;
        let done = report.deleted + report.let_go;
        if done + report.failed < batch as usize || done == 0 {
            return total;
        }
    }
}

/// What a job's guarded terminal write does when it finds no row: the files
/// its delivery stored just before (`result_url`, `metadata.assets`) belong
/// to a generation that is gone. When its app (or, with no app, its org) is
/// gone too, deleted while the delivery ran, nothing would ever find them:
/// they are recorded for deletion as the delete's own are, and the workers
/// delete them. A row missing while its app still stands is left alone.
pub async fn record_files_of_a_vanished_generation(
    db: &dyn DatabaseStore,
    generation_id: &str,
    org_id: Option<&str>,
    app_id: Option<&str>,
    result_url: Option<&str>,
    metadata: Option<&serde_json::Value>,
) {
    let Some((scope, tenant)) = vanished_tenant(db, generation_id, org_id, app_id).await else { return };
    let metadata = metadata.map(|m| m.to_string());
    let urls = crate::db::kept_file_urls(result_url, metadata.as_deref());
    record_files(db, scope, tenant, generation_id, &urls).await;
}

/// What every request does once it has written its rows: its request-log
/// row `request_id` (with its tags and artifact), the generation
/// `generation_id` if it made one, and the files it `kept` in litegen's
/// store. When its app (or, with no app, its org: `key_ctx`'s) was deleted
/// while it ran, those rows would outlive the tenant: they are removed, and
/// the files recorded for deletion. Nothing happens while the app stands.
/// Returns whether the tenant was gone.
///
/// It looks the tenant up, except for a request that made no generation on a
/// database whose keys tie every row a request writes to its tenant
/// ([`DatabaseStore::request_rows_reference_their_tenant`]: Postgres). There
/// a row written after the delete is refused, one written before it is
/// deleted with it, and one written while it runs makes it start again and
/// take it, so the lookup could only find nothing to do. A request that made
/// a generation still looks: a delete between its row's insert and the write
/// that completes it finds no files on the row to record, and a job
/// submitted just before the delete is failed and let go at its vendor.
pub async fn forget_a_request_of_a_vanished_tenant(
    db: &dyn DatabaseStore,
    key_ctx: Option<&KeyContext>,
    request_id: &str,
    generation_id: Option<&str>,
    kept: &[String],
) -> bool {
    if generation_id.is_none() && db.request_rows_reference_their_tenant() {
        return false;
    }
    let (org_id, app_id) = key_ctx.map_or((None, None), |c| (c.org_id.as_deref(), c.app_id.as_deref()));
    let Some((scope, tenant)) = vanished_tenant(db, request_id, org_id, app_id).await else { return false };
    if let Err(e) = db.forget_request_rows(generation_id, request_id).await {
        warn!(request_id, error = %e, "a request finished after its org or app was deleted, and its rows could not be removed");
    }
    if let Some(generation_id) = generation_id {
        record_files(db, scope, tenant, generation_id, kept).await;
    }
    true
}

/// `(scope, id)` of the app (with no app, the org) named, when it no longer
/// exists; `None` while it stands, when none is named, or when that can't
/// be read (logged).
async fn vanished_tenant<'a>(db: &dyn DatabaseStore, of: &str, org_id: Option<&'a str>, app_id: Option<&'a str>) -> Option<(&'static str, &'a str)> {
    let gone = match (app_id, org_id) {
        (Some(app), _) => db.get_application(app).await.map(|found| found.is_none().then_some(("app", app))),
        (None, Some(org)) => db.get_organization(org).await.map(|found| found.is_none().then_some(("org", org))),
        (None, None) => Ok(None),
    };
    gone.unwrap_or_else(|e| {
        warn!(id = of, error = %e, "whether an org or app was deleted can't be read; what it wrote stays");
        None
    })
}

/// Record `urls` of `generation_id`, of deleted `scope` `tenant`, for deletion.
async fn record_files(db: &dyn DatabaseStore, scope: &str, tenant: &str, generation_id: &str, urls: &[String]) {
    if urls.is_empty() {
        return;
    }
    match db.record_tenant_file_deletions(scope, tenant, generation_id, urls).await {
        Ok(()) => info!(generation_id, scope, tenant_id = tenant, files = urls.len(), "a job finished after its org or app was deleted: its files are recorded for deletion"),
        Err(e) => warn!(generation_id, error = %e, "a job finished after its org or app was deleted, and its files could not be recorded for deletion"),
    }
}

/// After a delete that committed: start on the files it recorded for
/// `scope` (`org` or `app`) `id`, on a task of its own. A pass that is cut or
/// whose deletes fail leaves the rest to the workers.
pub fn start_deleting(db: Arc<dyn DatabaseStore>, store: Arc<dyn ImageStorage>, scope: &str, id: &str) {
    let (scope, id) = (scope.to_string(), id.to_string());
    tokio::spawn(async move {
        let pass = delete_all_due(&*db, &*store, Some((&scope, &id)), BATCH);
        let report = crate::supervise::guarded("deleting a deleted org's or app's files", pass).await;
        if let Some(report) = report.filter(|r| *r != DeletionReport::default()) {
            info!(scope = %scope, tenant_id = %id, deleted = report.deleted, let_go = report.let_go, failed = report.failed, "deleted org or app: its files in litegen's store");
        }
    });
}

/// Every replica's worker: a pass over the due files of every deleted org
/// and app at once and every [`TICK`], until `shutdown`. A pass that panics
/// is logged and the next runs at its time; one cut by the stop leaves its
/// rows due again once their hold passes.
pub fn spawn_tenant_file_deleter(
    db: Arc<dyn DatabaseStore>,
    store: Arc<dyn ImageStorage>,
    shutdown: impl std::future::Future<Output = ()> + Send + 'static,
) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let mut shutdown = std::pin::pin!(shutdown);
        let mut tick = tokio::time::interval(TICK);
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Delay);
        loop {
            tokio::select! {
                biased;
                _ = &mut shutdown => return,
                _ = tick.tick() => {}
            }
            let pass = crate::supervise::guarded("a pass over deleted orgs' and apps' files", delete_all_due(&*db, &*store, None, BATCH));
            tokio::select! {
                biased;
                _ = &mut shutdown => return,
                report = pass => {
                    if let Some(report) = report.filter(|r| *r != DeletionReport::default()) {
                        info!(deleted = report.deleted, let_go = report.let_go, failed = report.failed, "deleted orgs and apps: a pass over their files in litegen's store");
                    }
                }
            }
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::collections::HashMap;
    use std::sync::Mutex;

    use crate::db::sqlite::SqliteDatabase;
    use crate::proxy::storage::{ImageStoreError, LocalModel3dStorage};
    use crate::types::{Application, Organization};

    /// litegen's local store, whose next `fail` deletes fail (an outage), and
    /// which counts the deletes of each key.
    struct Flaky {
        inner: LocalModel3dStorage,
        fail: Mutex<usize>,
        deletes: Mutex<HashMap<String, usize>>,
    }

    impl Flaky {
        fn new(dir: &std::path::Path, fail: usize) -> Self {
            Self { inner: LocalModel3dStorage::new(dir, "http://127.0.0.1:4000".into()), fail: Mutex::new(fail), deletes: Mutex::default() }
        }

        fn deletes_of(&self, key: &str) -> usize {
            self.deletes.lock().unwrap().get(key).copied().unwrap_or(0)
        }
    }

    #[async_trait::async_trait]
    impl ImageStorage for Flaky {
        async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, ImageStoreError> {
            self.inner.put(key, bytes, content_type).await
        }
        async fn delete(&self, key: &str) -> Result<(), ImageStoreError> {
            *self.deletes.lock().unwrap().entry(key.to_string()).or_default() += 1;
            let refused = {
                let mut fail = self.fail.lock().unwrap();
                let refused = *fail > 0;
                *fail = fail.saturating_sub(1);
                refused
            };
            if refused {
                return Err(ImageStoreError::UploadFailed("S3 DELETE returned HTTP 503".into()));
            }
            self.inner.delete(key).await
        }
        fn key_of(&self, url: &str) -> Option<String> {
            self.inner.key_of(url)
        }
    }

    /// An app in an org of its own, and `n` finished generations of it, each
    /// with one file kept in `store`: (org, app, the files' keys).
    async fn app_with_files(db: &SqliteDatabase, store: &dyn ImageStorage, tag: &str, n: usize) -> (String, String, Vec<String>) {
        let now = chrono::Utc::now();
        let org = Organization { id: format!("org-{tag}"), name: tag.into(), slug: format!("org-{tag}"), plan: "free".into(), status: "active".into(), created_at: now, updated_at: now };
        db.create_organization(&org).await.unwrap();
        let app = Application { id: format!("app-{tag}"), org_id: org.id.clone(), name: tag.into(), slug: format!("app-{tag}"), status: "active".into(), created_at: now, updated_at: now };
        db.create_application(&app).await.unwrap();
        let mut keys = Vec::new();
        for i in 0..n {
            let id = format!("g-{tag}-{i}");
            let key = format!("litegen/3d/{id}/model.glb");
            let url = store.put(&key, &bytes::Bytes::from_static(b"glTF-ish"), "model/gltf-binary").await.unwrap();
            finished(db, &id, &org.id, &app.id, &url).await;
            keys.push(key);
        }
        (org.id, app.id, keys)
    }

    /// A finished generation of `app` whose one asset is at `url`.
    async fn finished(db: &SqliteDatabase, id: &str, org: &str, app: &str, url: &str) {
        db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", None, 0.0, Some(org), Some(app)).await.unwrap();
        let meta = serde_json::json!({ "assets": [{ "index": 0, "kind": "mesh", "format": "glb", "url": url }] });
        db.update_generation_if_active(id, "completed", 100, Some(url), None, Some(chrono::Utc::now()), Some(&meta)).await.unwrap();
    }

    /// Time passes: every file deletion is due now.
    async fn due_now(db: &SqliteDatabase) {
        sqlx::query("UPDATE tenant_file_deletions SET next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', 'now', '-1 seconds')")
            .execute(db.pool())
            .await
            .unwrap();
    }

    async fn pending(db: &SqliteDatabase) -> i64 {
        sqlx::query_scalar("SELECT COUNT(*) FROM tenant_file_deletions").fetch_one(db.pool()).await.unwrap()
    }

    #[tokio::test]
    async fn a_deleted_apps_files_are_deleted_even_when_the_first_pass_fails() {
        let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Flaky::new(dir.path(), 2);
        let (_, app, keys) = app_with_files(&db, &store, "flaky", 2).await;
        assert!(db.delete_application(&app).await.unwrap());

        // The pass right after the delete: the store refuses.
        let first = delete_due(&db, &store, Some(("app", &app)), BATCH).await;
        assert_eq!(first, DeletionReport { deleted: 0, let_go: 0, failed: 2 });
        assert!(keys.iter().all(|k| dir.path().join(k).exists()), "the files are still there");
        let (attempts, error, waits): (i64, Option<String>, bool) = sqlx::query_as(
            "SELECT MAX(attempts), MAX(last_error), MIN(next_attempt_at) > strftime('%Y-%m-%d %H:%M:%f', 'now', '+50 seconds') FROM tenant_file_deletions",
        )
        .fetch_one(db.pool())
        .await
        .unwrap();
        assert_eq!((attempts, waits), (1, true), "tried once, due again in a minute");
        assert!(error.is_some_and(|e| e.contains("503")), "the error is kept");
        assert_eq!(delete_due(&db, &store, None, BATCH).await, DeletionReport::default(), "not due again yet");

        // Later, the store answers again.
        due_now(&db).await;
        assert_eq!(delete_due(&db, &store, None, BATCH).await, DeletionReport { deleted: 2, let_go: 0, failed: 0 });
        assert!(keys.iter().all(|k| !dir.path().join(k).exists()), "deleted");
        assert_eq!(pending(&db).await, 0, "and their records with them");
    }

    #[tokio::test]
    async fn a_worker_that_dies_mid_way_leaves_its_files_to_another() {
        let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Flaky::new(dir.path(), 0);
        let (_, app, keys) = app_with_files(&db, &store, "dies", 3).await;
        assert!(db.delete_application(&app).await.unwrap());

        // A worker claims all three, deletes the first file, and dies before
        // it removes any record.
        let hold = chrono::Duration::from_std(HOLD).unwrap();
        assert_eq!(db.claim_tenant_file_deletions(None, BATCH, hold).await.unwrap().len(), 3);
        store.delete(&keys[0]).await.unwrap();

        // While its hold lasts, no other worker takes them.
        assert_eq!(delete_due(&db, &store, None, BATCH).await, DeletionReport::default());
        assert!(dir.path().join(&keys[1]).exists());

        // Its hold passes: another worker deletes all three, the first one
        // again, which is gone already and no error.
        due_now(&db).await;
        assert_eq!(delete_due(&db, &store, None, BATCH).await, DeletionReport { deleted: 3, let_go: 0, failed: 0 });
        assert!(keys.iter().all(|k| !dir.path().join(k).exists()));
        assert_eq!(pending(&db).await, 0);
        let deletes: Vec<usize> = keys.iter().map(|k| store.deletes_of(k)).collect();
        assert_eq!(deletes, [2, 1, 1], "the first one twice, harmlessly");
    }

    #[tokio::test]
    async fn two_workers_at_once_never_take_the_same_file() {
        let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Flaky::new(dir.path(), 0);
        let (org, _, keys) = app_with_files(&db, &store, "busy", 40).await;
        assert!(db.delete_organization(&org).await.unwrap());

        let (a, b) = tokio::join!(delete_due(&db, &store, None, 25), delete_due(&db, &store, None, 25));
        assert_eq!((a.deleted + b.deleted, a.failed + b.failed, a.let_go + b.let_go), (40, 0, 0), "{a:?} {b:?}");
        assert!(keys.iter().all(|k| store.deletes_of(k) == 1), "each file deleted once");
        assert_eq!(pending(&db).await, 0);
    }

    /// litegen's local store, each of whose deletes takes as long as one may
    /// ([`DELETE_TIMEOUT`]: the database's clock moves on by that much), while
    /// another worker claims what is due. It keeps the keys each side took.
    struct SlowStore {
        inner: LocalModel3dStorage,
        db: Arc<SqliteDatabase>,
        deleted: Mutex<Vec<String>>,
        taken_by_another: Mutex<Vec<String>>,
    }

    #[async_trait::async_trait]
    impl ImageStorage for SlowStore {
        async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, ImageStoreError> {
            self.inner.put(key, bytes, content_type).await
        }
        async fn delete(&self, key: &str) -> Result<(), ImageStoreError> {
            self.deleted.lock().unwrap().push(key.to_string());
            sqlx::query("UPDATE tenant_file_deletions SET next_attempt_at = strftime('%Y-%m-%d %H:%M:%f', next_attempt_at, ?)")
                .bind(format!("-{} seconds", DELETE_TIMEOUT.as_secs()))
                .execute(self.db.pool())
                .await
                .unwrap();
            let hold = chrono::Duration::from_std(HOLD).unwrap();
            let taken = self.db.claim_tenant_file_deletions(None, BATCH, hold).await.unwrap();
            self.taken_by_another.lock().unwrap().extend(taken.iter().filter_map(|d| self.inner.key_of(&d.url)));
            self.inner.delete(key).await
        }
        fn key_of(&self, url: &str) -> Option<String> {
            self.inner.key_of(url)
        }
    }

    /// A batch's deletes end within its hold, even on a store that takes as
    /// long as it may over each one: no other worker takes a file of the
    /// batch meanwhile, to delete it again and lose the first worker's record
    /// of a failure.
    #[tokio::test]
    async fn a_slow_stores_batch_ends_before_another_worker_can_take_its_files() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let dir = tempfile::tempdir().unwrap();
        let store = SlowStore {
            inner: LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()),
            db: db.clone(),
            deleted: Mutex::default(),
            taken_by_another: Mutex::default(),
        };
        let (org, _, _) = app_with_files(&db, &store, "slow", 10).await;
        assert!(db.delete_organization(&org).await.unwrap());

        let report = delete_due(&*db, &store, None, BATCH).await;
        let deleted = store.deleted.lock().unwrap().clone();
        let taken = store.taken_by_another.lock().unwrap().clone();
        assert_eq!((report.deleted, report.failed), (deleted.len(), 0), "{report:?}");
        let both: Vec<&String> = deleted.iter().filter(|k| taken.contains(k)).collect();
        assert!(both.is_empty(), "another worker took files of the batch while it ran: {both:?}");
    }

    /// A file in the app's own bucket is the customer's: it stays, as before.
    #[tokio::test]
    async fn a_file_outside_litegens_store_is_let_go_untouched() {
        let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Flaky::new(dir.path(), 0);
        let (org, app, keys) = app_with_files(&db, &store, "byo", 1).await;
        finished(&db, "g-in-their-bucket", &org, &app, "https://customer-bucket.s3.us-east-1.amazonaws.com/litegen/3d/g-in-their-bucket/model.glb").await;
        assert!(db.delete_application(&app).await.unwrap());

        assert_eq!(delete_due(&db, &store, None, BATCH).await, DeletionReport { deleted: 1, let_go: 1, failed: 0 });
        assert!(!dir.path().join(&keys[0]).exists());
        assert_eq!(store.deletes_of("litegen/3d/g-in-their-bucket/model.glb"), 0, "nothing deleted for the bucket's file");
        assert_eq!(pending(&db).await, 0);
    }

    /// One call works off every due file, a batch at a time, while its
    /// batches delete; a batch whose deletes all fail ends it, and the rest
    /// wait for the next pass.
    #[tokio::test]
    async fn one_call_works_off_every_due_file_a_batch_at_a_time() {
        let db = SqliteDatabase::connect("sqlite::memory:").await.unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = Flaky::new(dir.path(), 0);
        let (_, app, keys) = app_with_files(&db, &store, "many", 5).await;
        assert!(db.delete_application(&app).await.unwrap());
        assert_eq!(delete_all_due(&db, &store, Some(("app", &app)), 2).await, DeletionReport { deleted: 5, let_go: 0, failed: 0 });
        assert!(keys.iter().all(|k| !dir.path().join(k).exists()));

        let refusing = Flaky::new(dir.path(), usize::MAX);
        let (_, app, _) = app_with_files(&db, &refusing, "refused", 5).await;
        assert!(db.delete_application(&app).await.unwrap());
        assert_eq!(delete_all_due(&db, &refusing, None, 2).await, DeletionReport { deleted: 0, let_go: 0, failed: 2 });
        assert_eq!(pending(&db).await, 5, "all five still to do");
    }

    #[test]
    fn the_wait_after_a_failed_delete_doubles_up_to_six_hours() {
        assert_eq!(retry_delay(0), chrono::Duration::minutes(1));
        assert_eq!(retry_delay(1), chrono::Duration::minutes(1));
        assert_eq!(retry_delay(2), chrono::Duration::minutes(2));
        assert_eq!(retry_delay(4), chrono::Duration::minutes(8));
        assert_eq!(retry_delay(9), chrono::Duration::minutes(256));
        assert_eq!(retry_delay(10), chrono::Duration::hours(6));
        assert_eq!(retry_delay(i32::MAX), chrono::Duration::hours(6));
    }
}

/// What happens around a tenant's delete: a job that finishes delivering
/// after its app was deleted, a delete whose caller hangs up, and a failed
/// request's log row whose request is dropped mid-write.
#[cfg(test)]
mod after_the_delete_tests {
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Arc;
    use std::time::Duration;

    use axum::body::Body;
    use axum::extract::{Path, State};
    use axum::http::{Request, StatusCode};
    use axum::Extension;
    use tokio::sync::Notify;
    use tower::ServiceExt;

    use crate::api::middleware::{AppState, KeyContext, DEFAULT_APP_ID};
    use crate::db::sqlite::SqliteDatabase;
    use crate::db::DatabaseStore;
    use crate::proxy::storage::{ImageStorage, ImageStoreError, LocalModel3dStorage};
    use crate::types::*;

    struct NoTemp;

    #[async_trait::async_trait]
    impl crate::proxy::materializer::TempStorage for NoTemp {
        async fn put(&self, key: &str, _bytes: bytes::Bytes, _ct: &str) -> Result<String, crate::proxy::materializer::MaterializeError> {
            Ok(format!("local://{key}"))
        }
        async fn delete(&self, _key: &str) -> Result<(), crate::proxy::materializer::MaterializeError> {
            Ok(())
        }
    }

    /// A single-tenant state over `db`, with the mock vendors, whose own
    /// store (litegen's) is `store`.
    async fn state_over(db: Arc<dyn DatabaseStore>, store: Arc<dyn ImageStorage>) -> Arc<AppState> {
        state_over_with(db, store, None, |_| {}).await
    }

    /// The master key of the states that take API keys: without one, a
    /// single-tenant deployment runs every request as its dev principal, in
    /// the default org and app, whatever key it bears.
    const MASTER_KEY: &str = "sk-tenant-files-master";

    /// [`state_over`], with `master_key` (API keys are then read), and the
    /// config tweaked first.
    async fn state_over_with(
        db: Arc<dyn DatabaseStore>,
        store: Arc<dyn ImageStorage>,
        master_key: Option<&str>,
        tweak: impl FnOnce(&mut crate::config::AppConfig),
    ) -> Arc<AppState> {
        let mut config = crate::config::AppConfig::default();
        tweak(&mut config);
        config.providers.insert("mock".to_string(), serde_json::from_value(serde_json::json!({ "enabled": true })).unwrap());
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.init_from_config(&config).await;
        let cache = Arc::new(crate::proxy::cache::GenerationCache::new(&crate::config::CacheGlobalConfig::default()));
        let mut router = crate::proxy::router::ProxyRouter::new(registry, cache, Arc::new(config), Arc::new(crate::proxy::storage::LocalStore));
        router.model3d_store = store;
        let mut models = std::path::PathBuf::from(env!("CARGO_MANIFEST_DIR"));
        models.pop();
        models.push("models");
        Arc::new(AppState {
            router: Arc::new(router),
            db,
            master_key: master_key.map(str::to_string),
            registry: Arc::new(crate::capabilities::CapabilityRegistry::from_dir(&models).expect("models")),
            materializer: Arc::new(crate::proxy::materializer::Materializer::new(Arc::new(NoTemp), reqwest::Client::new())),
            rate_limiter: Arc::new(crate::api::middleware::rate_limit::RateLimiter::new()),
            in_flight: Arc::new(crate::api::middleware::backpressure::InFlightLimit::new(64)),
            oauth: crate::auth::oauth::OAuthConfig::default(),
            mode: crate::config::Mode::SingleTenant,
            secrets_key: None,
            dev: crate::config::DevFlags::default(),
            allow_password: true,
        })
    }

    async fn call(app: &axum::Router, method: &str, uri: &str, body: Option<serde_json::Value>) -> (StatusCode, serde_json::Value) {
        let req = Request::builder().method(method).uri(uri);
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let status = resp.status();
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
        (status, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    /// The files under `dir` whose path names `id`.
    fn files_of(dir: &std::path::Path, id: &str) -> Vec<std::path::PathBuf> {
        let mut found = Vec::new();
        let mut dirs = vec![dir.to_path_buf()];
        while let Some(d) = dirs.pop() {
            for e in std::fs::read_dir(&d).into_iter().flatten().flatten() {
                let path = e.path();
                if path.is_dir() {
                    dirs.push(path);
                } else if path.to_string_lossy().contains(id) {
                    found.push(path);
                }
            }
        }
        found
    }

    /// An org of its own and an app of it: (org, app).
    async fn tenant(db: &dyn DatabaseStore, tag: &str) -> (String, String) {
        let now = chrono::Utc::now();
        let org = Organization { id: format!("org-{tag}"), name: tag.into(), slug: format!("org-{tag}"), plan: "free".into(), status: "active".into(), created_at: now, updated_at: now };
        db.create_organization(&org).await.unwrap();
        let app = Application { id: format!("app-{tag}"), org_id: org.id.clone(), name: tag.into(), slug: format!("app-{tag}"), status: "active".into(), created_at: now, updated_at: now };
        db.create_application(&app).await.unwrap();
        (org.id, app.id)
    }

    /// The master key's context: it may delete any org or app.
    fn platform_admin() -> KeyContext {
        KeyContext {
            key_id: None,
            scopes: Vec::new(),
            rpm_limit: None,
            quota_remaining: None,
            webhook_url: None,
            user: None,
            permissions: Vec::new(),
            session_id: None,
            org_id: None,
            app_id: None,
            oauth_grant_id: None,
        }
    }

    /// litegen's local store, which deletes app `app` just before it stores
    /// its first file: the app is deleted while a delivery stores what its
    /// job made.
    struct DeletesTheAppFirst {
        inner: LocalModel3dStorage,
        db: Arc<dyn DatabaseStore>,
        app: String,
        deleted: AtomicBool,
    }

    impl DeletesTheAppFirst {
        fn new(dir: &std::path::Path, db: Arc<dyn DatabaseStore>, app: &str) -> Self {
            Self { inner: LocalModel3dStorage::new(dir, "http://127.0.0.1:4000".into()), db, app: app.to_string(), deleted: AtomicBool::new(false) }
        }

        async fn delete_the_app_once(&self) {
            if !self.deleted.swap(true, Ordering::SeqCst) {
                assert!(self.db.delete_application(&self.app).await.unwrap(), "the app was there to delete");
            }
        }
    }

    #[async_trait::async_trait]
    impl ImageStorage for DeletesTheAppFirst {
        async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, ImageStoreError> {
            self.delete_the_app_once().await;
            self.inner.put(key, bytes, content_type).await
        }
        async fn put_file(&self, key: &str, path: &std::path::Path, content_type: &str) -> Result<String, ImageStoreError> {
            self.delete_the_app_once().await;
            self.inner.put_file(key, path, content_type).await
        }
        async fn delete(&self, key: &str) -> Result<(), ImageStoreError> {
            self.inner.delete(key).await
        }
        async fn get(&self, key: &str) -> Result<Option<(bytes::Bytes, String)>, ImageStoreError> {
            self.inner.get(key).await
        }
        fn key_of(&self, url: &str) -> Option<String> {
            self.inner.key_of(url)
        }
    }

    /// A 3D vendor whose job is done at its first poll, with one GLB.
    struct DoneMesh;

    #[async_trait::async_trait]
    impl crate::providers::Model3dProvider for DoneMesh {
        fn name(&self) -> &str {
            "mock"
        }
        fn configure(&mut self, _c: crate::providers::ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool {
            true
        }
        async fn generate(
            &self,
            m: &crate::capabilities::ModelSchema,
            _b: &BaseGenerationRequest,
            _e: &crate::providers::Model3dExtras,
            _mat: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<crate::providers::Model3dGenerationHandle, crate::providers::ProviderError> {
            Ok(crate::providers::Model3dGenerationHandle { provider_job_id: "done-job".into(), provider: "mock".into(), model: m.id.clone(), stage_context: None })
        }
        async fn poll_status(
            &self,
            _h: &crate::providers::Model3dGenerationHandle,
        ) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
            Ok(crate::providers::Model3dGenerationPollResult {
                status: GenerationStatus::Completed,
                progress: 100,
                files: vec![crate::providers::Model3dFile {
                    kind: Model3dAssetKind::Mesh,
                    format: "glb".into(),
                    content_type: "model/gltf-binary".into(),
                    bytes: crate::providers::model3d::glb::generate_cube_glb("late"),
                    polycount: None,
                    width: None,
                    height: None,
                    manifest: None,
                    source_url: None,
                    size_bytes: None,
                }],
                error: None,
                metadata: Default::default(),
                stage_context: None,
                next_stage: None,
            })
        }
        async fn estimate_cost(
            &self,
            _m: &crate::capabilities::ModelSchema,
            _r: &Model3dGenerationRequest,
        ) -> Result<CostEstimate, crate::providers::ProviderError> {
            unimplemented!()
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            unimplemented!()
        }
    }

    /// The database, with deleting an app or an org, and writing a request
    /// log row, each held until the test lets it on: `entered` when one
    /// starts, `release` lets it on, `done` once the real write has run.
    struct Held {
        inner: Arc<SqliteDatabase>,
        entered: Notify,
        release: Notify,
        done: Notify,
    }

    impl Held {
        fn new(inner: Arc<SqliteDatabase>) -> Self {
            Self { inner, entered: Notify::new(), release: Notify::new(), done: Notify::new() }
        }

        async fn hold<T>(&self, write: impl std::future::Future<Output = T>) -> T {
            self.entered.notify_one();
            self.release.notified().await;
            let out = write.await;
            self.done.notify_one();
            out
        }
    }

    #[async_trait::async_trait]
    impl DatabaseStore for Held {
        async fn delete_application(&self, id: &str) -> Result<bool, sqlx::Error> {
            self.hold(self.inner.delete_application(id)).await
        }
        async fn delete_organization(&self, id: &str) -> Result<bool, sqlx::Error> {
            self.hold(self.inner.delete_organization(id)).await
        }
        async fn log_request(
            &self, id: &str, model: &str, provider: &str, status: &str, media_type: &str, cost_usd: f64, latency_ms: i64,
            error: Option<&str>, metadata: Option<&serde_json::Value>, org_id: Option<&str>, app_id: Option<&str>,
        ) -> Result<(), sqlx::Error> {
            self.hold(self.inner.log_request(id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, org_id, app_id)).await
        }
        async fn get_application(&self, id: &str) -> Result<Option<Application>, sqlx::Error> {
            self.inner.get_application(id).await
        }
        async fn get_organization(&self, id: &str) -> Result<Option<Organization>, sqlx::Error> {
            self.inner.get_organization(id).await
        }
        async fn list_active_generations_for_tenant(&self, org_id: &str, app_id: Option<&str>) -> Result<Vec<Generation>, sqlx::Error> {
            self.inner.list_active_generations_for_tenant(org_id, app_id).await
        }
        async fn insert_request_artifact(&self, a: &RequestArtifact) -> Result<(), sqlx::Error> {
            self.inner.insert_request_artifact(a).await
        }
        async fn insert_generation(
            &self, id: &str, key_id: Option<&uuid::Uuid>, model: &str, provider: &str, media_type: &str,
            provider_job_id: Option<&str>, cost_usd: f64, org_id: Option<&str>, app_id: Option<&str>,
        ) -> Result<(), sqlx::Error> {
            self.inner.insert_generation(id, key_id, model, provider, media_type, provider_job_id, cost_usd, org_id, app_id).await
        }
        async fn update_generation_status(
            &self, id: &str, status: &str, progress: i32, result_url: Option<&str>, error: Option<&str>,
            completed_at: Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<(), sqlx::Error> {
            self.inner.update_generation_status(id, status, progress, result_url, error, completed_at).await
        }
        async fn update_generation_metadata(&self, id: &str, metadata: &serde_json::Value) -> Result<(), sqlx::Error> {
            self.inner.update_generation_metadata(id, metadata).await
        }
        async fn update_request_log_status(&self, id: &str, status: &str, error: Option<&str>) -> Result<(), sqlx::Error> {
            self.inner.update_request_log_status(id, status, error).await
        }
        async fn get_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error> {
            self.inner.get_generation(id).await
        }
        async fn list_active_generations(&self, limit: u32) -> Result<Vec<Generation>, sqlx::Error> {
            self.inner.list_active_generations(limit).await
        }
        async fn list_generations(&self, key_id: Option<&uuid::Uuid>, page: u32, per_page: u32) -> Result<Vec<Generation>, sqlx::Error> {
            self.inner.list_generations(key_id, page, per_page).await
        }
        async fn count_generations(&self, key_id: Option<&uuid::Uuid>) -> Result<i64, sqlx::Error> {
            self.inner.count_generations(key_id).await
        }
        async fn cancel_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error> {
            self.inner.cancel_generation(id).await
        }
        async fn get_request_logs(&self, page: u32, per_page: u32) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
            self.inner.get_request_logs(page, per_page).await
        }
        async fn get_request_logs_filtered(&self, filters: &LogFilters, page: u32, per_page: u32) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
            self.inner.get_request_logs_filtered(filters, page, per_page).await
        }
        async fn create_api_key(
            &self, name: &str, key_hash: &str, key_prefix: &str, token_quota: Option<f64>, rpm_limit: Option<u32>, scopes: &str,
            webhook_url: Option<&str>,
        ) -> Result<ApiKey, sqlx::Error> {
            self.inner.create_api_key(name, key_hash, key_prefix, token_quota, rpm_limit, scopes, webhook_url).await
        }
        async fn get_api_key(&self, id: &uuid::Uuid) -> Result<Option<ApiKey>, sqlx::Error> {
            self.inner.get_api_key(id).await
        }
        async fn update_api_key(&self, id: &uuid::Uuid, req: &UpdateApiKeyRequest) -> Result<Option<ApiKey>, sqlx::Error> {
            self.inner.update_api_key(id, req).await
        }
        async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
            self.inner.lookup_api_key_by_hash(key_hash).await
        }
        async fn atomic_charge_tokens(&self, id: &uuid::Uuid, cost_usd: f64) -> Result<f64, sqlx::Error> {
            self.inner.atomic_charge_tokens(id, cost_usd).await
        }
        async fn validate_api_key(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
            self.inner.validate_api_key(key_hash).await
        }
        async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error> {
            self.inner.list_api_keys().await
        }
        async fn revoke_api_key(&self, id: &uuid::Uuid) -> Result<bool, sqlx::Error> {
            self.inner.revoke_api_key(id).await
        }
        async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error> {
            self.inner.get_stats().await
        }
    }

    // ─── a job delivered after its app was deleted ──────────────────────

    #[tokio::test]
    async fn a_job_the_poller_delivers_after_its_app_was_deleted_has_its_files_deleted() {
        let db: Arc<dyn DatabaseStore> = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let (org, app) = tenant(&*db, "poller-late").await;
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(DeletesTheAppFirst::new(dir.path(), db.clone(), &app));
        let registry = Arc::new(crate::proxy::registry::ProviderRegistry::new());
        registry.register_mock_model3d(Arc::new(DoneMesh)).await;
        let id = "litegen-3d-delivered-late";
        db.insert_generation(id, None, "mock/mesh-3d", "mock", "model3d", Some("done-job"), 0.0, Some(&org), Some(&app)).await.unwrap();

        crate::proxy::poller::poll_once(&db, &registry, None, crate::config::Mode::SingleTenant, &store).await;

        assert!(!files_of(dir.path(), id).is_empty(), "the delivery stored its mesh after the app was deleted");
        assert!(db.get_generation(id).await.unwrap().is_none(), "the row went with the app");
        let report = super::delete_all_due(&*db, &*store, None, super::BATCH).await;
        assert!(files_of(dir.path(), id).is_empty(), "recorded, so deleted: {report:?}");
    }

    #[tokio::test]
    async fn a_3d_job_a_client_polls_after_its_app_was_deleted_has_its_files_deleted() {
        let db: Arc<dyn DatabaseStore> = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(DeletesTheAppFirst::new(dir.path(), db.clone(), DEFAULT_APP_ID));
        let app = crate::api::handlers::create_router(state_over(db.clone(), store.clone()).await);
        let (s, v) = call(&app, "POST", "/v1/models3d/generations", Some(serde_json::json!({ "model": "mock/mesh-3d", "prompt": "a cube" }))).await;
        assert!(s.is_success(), "{s} {v}");
        let id = v["id"].as_str().unwrap().to_string();
        // The client polls until its job ends, or its row is gone with the app.
        for _ in 0..40 {
            let (s, v) = call(&app, "GET", &format!("/v1/models3d/{id}"), None).await;
            if s == StatusCode::NOT_FOUND || v["status"] == "completed" || v["status"] == "failed" {
                break;
            }
        }

        assert!(!files_of(dir.path(), &id).is_empty(), "the delivery stored its mesh after the app was deleted");
        assert!(db.get_generation(&id).await.unwrap().is_none(), "the row went with the app");
        let report = super::delete_all_due(&*db, &*store, None, super::BATCH).await;
        assert!(files_of(dir.path(), &id).is_empty(), "recorded, so deleted: {report:?}");
    }

    #[tokio::test]
    async fn a_video_a_client_polls_after_its_app_was_deleted_has_its_files_deleted() {
        let db: Arc<dyn DatabaseStore> = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(DeletesTheAppFirst::new(dir.path(), db.clone(), DEFAULT_APP_ID));
        let app = crate::api::handlers::create_router(state_over(db.clone(), store.clone()).await);
        let (s, v) = call(&app, "POST", "/v1/videos/generations", Some(serde_json::json!({ "model": "mock/video-gen", "prompt": "a wave" }))).await;
        assert!(s.is_success(), "{s} {v}");
        let id = v["id"].as_str().unwrap().to_string();
        for _ in 0..40 {
            let (s, v) = call(&app, "GET", &format!("/v1/videos/{id}"), None).await;
            if s == StatusCode::NOT_FOUND || v["status"] == "completed" || v["status"] == "failed" {
                break;
            }
        }

        assert!(!files_of(dir.path(), &id).is_empty(), "the delivery kept the video after the app was deleted");
        assert!(db.get_generation(&id).await.unwrap().is_none(), "the row went with the app");
        let report = super::delete_all_due(&*db, &*store, None, super::BATCH).await;
        assert!(files_of(dir.path(), &id).is_empty(), "recorded, so deleted: {report:?}");
    }

    /// A row missing while its app still stands isn't a deleted tenant's:
    /// nothing is recorded, so its files are never deleted this way. Once the
    /// app is gone, the same files are.
    #[tokio::test]
    async fn a_missing_row_whose_app_still_stands_records_nothing() {
        let db: Arc<dyn DatabaseStore> = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let (org, app) = tenant(&*db, "still-here").await;
        let meta = serde_json::json!({ "assets": [{ "kind": "mesh", "url": "http://127.0.0.1:4000/v1/models3d/assets/litegen/3d/g-not-yet/model.glb" }] });
        let hold = chrono::Duration::seconds(60);
        super::record_files_of_a_vanished_generation(&*db, "g-not-yet", Some(&org), Some(&app), None, Some(&meta)).await;
        assert!(db.claim_tenant_file_deletions(None, 100, hold).await.unwrap().is_empty(), "the app stands: nothing recorded");

        assert!(db.delete_application(&app).await.unwrap());
        super::record_files_of_a_vanished_generation(&*db, "g-not-yet", Some(&org), Some(&app), None, Some(&meta)).await;
        let recorded = db.claim_tenant_file_deletions(Some(("app", &app)), 100, hold).await.unwrap();
        assert_eq!(recorded.iter().map(|d| d.generation_id.as_str()).collect::<Vec<_>>(), ["g-not-yet"]);
    }

    /// A key of an org and an app of their own: (org, app, its secret).
    async fn tenant_with_key(db: &dyn DatabaseStore, tag: &str) -> (String, String, String) {
        use sha2::Digest;
        let (org, app) = tenant(db, tag).await;
        let secret = format!("sk-{tag}-{}", uuid::Uuid::new_v4().simple());
        let hash = hex::encode(sha2::Sha256::digest(secret.as_bytes()));
        let public = format!("pk_{}", uuid::Uuid::new_v4().simple());
        db.create_api_key_scoped(&org, &app, &public, tag, &hash, "sk-", None, None, "generate,read", None, None).await.unwrap();
        (org, app, secret)
    }

    async fn as_key(app: &axum::Router, method: &str, uri: &str, secret: &str, body: Option<serde_json::Value>) -> (StatusCode, axum::http::HeaderMap, serde_json::Value) {
        let req = Request::builder().method(method).uri(uri).header("authorization", format!("Bearer {secret}"));
        let req = match body {
            Some(b) => req.header("content-type", "application/json").body(Body::from(b.to_string())).unwrap(),
            None => req.body(Body::empty()).unwrap(),
        };
        let resp = app.clone().oneshot(req).await.unwrap();
        let (status, headers) = (resp.status(), resp.headers().clone());
        let bytes = axum::body::to_bytes(resp.into_body(), 64 * 1024 * 1024).await.unwrap();
        (status, headers, serde_json::from_slice(&bytes).unwrap_or(serde_json::Value::Null))
    }

    // ─── a sync image whose app is deleted mid-request ──────────────────

    /// An image whose app is deleted while it is kept: the caller gets the
    /// image it paid for, and nothing of the deleted app's stays behind: no
    /// generation row, no request-log row, and the kept file recorded for
    /// deletion.
    async fn an_image_whose_app_is_deleted_mid_request_leaves_nothing_behind_on(db: Arc<dyn DatabaseStore>, tag: &str) {
        let (_, app, secret) = tenant_with_key(&*db, tag).await;
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(DeletesTheAppFirst::new(dir.path(), db.clone(), &app));
        let router = crate::api::handlers::create_router(state_over_with(db.clone(), store.clone(), Some(MASTER_KEY), |_| {}).await);

        let (status, _, body) = as_key(&router, "POST", "/v1/images/generations", &secret, Some(serde_json::json!({ "model": "mock/image-gen", "prompt": "a fox" }))).await;
        assert_eq!(status, StatusCode::OK, "the caller gets its image: {body}");
        let id = body["id"].as_str().unwrap().to_string();
        assert!(!files_of(dir.path(), &id).is_empty(), "the image was kept after its app was deleted");
        let left = db.get_generation(&id).await.unwrap();
        assert!(left.is_none(), "no generation row of the deleted app's: {:?} (app {app} still there: {:?})", left.map(|g| (g.org_id, g.app_id)), db.get_application(&app).await.unwrap().map(|a| a.id));
        let (rows, _) = db.get_request_logs(1, 100).await.unwrap();
        assert!(rows.iter().all(|r| r.id != id), "no request-log row of the deleted app's");
        let report = super::delete_all_due(&*db, &*store, Some(("app", &app)), super::BATCH).await;
        assert!(files_of(dir.path(), &id).is_empty(), "recorded, so deleted: {report:?}");
    }

    #[tokio::test]
    async fn an_image_whose_app_is_deleted_mid_request_leaves_nothing_behind() {
        let db: Arc<dyn DatabaseStore> = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        an_image_whose_app_is_deleted_mid_request_leaves_nothing_behind_on(db, "image-gone").await;
    }

    #[tokio::test]
    async fn an_image_whose_app_is_deleted_mid_request_leaves_nothing_behind_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping an_image_whose_app_is_deleted_mid_request_leaves_nothing_behind_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db: Arc<dyn DatabaseStore> = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        an_image_whose_app_is_deleted_mid_request_leaves_nothing_behind_on(db, &format!("image-gone-{}", uuid::Uuid::new_v4().simple())).await;
    }

    // ─── a 3D submit or a segmentation whose app is deleted at the vendor ─

    /// The database a test reads rows from, whichever the backend.
    enum Rows {
        Sqlite(Arc<SqliteDatabase>),
        Postgres(Arc<crate::db::PostgresDatabase>),
    }

    impl Rows {
        fn db(&self) -> Arc<dyn DatabaseStore> {
            match self {
                Rows::Sqlite(db) => db.clone(),
                Rows::Postgres(db) => db.clone(),
            }
        }

        /// The rows `sql` counts, `$1` bound to `v`.
        async fn count(&self, sql: &str, v: &str) -> i64 {
            match self {
                Rows::Sqlite(db) => sqlx::query_scalar(&sql.replace("$1", "?1")).bind(v).fetch_one(db.pool()).await.unwrap(),
                Rows::Postgres(db) => sqlx::query_scalar(sql).bind(v).fetch_one(db.pool()).await.unwrap(),
            }
        }

        /// Every row of app `app`'s, and of requests tagged `tag`, still there.
        async fn left_of(&self, app: &str, tag: &str) -> Vec<(&'static str, i64)> {
            vec![
                ("generations", self.count("SELECT COUNT(*) FROM generations WHERE app_id = $1", app).await),
                ("request_logs", self.count("SELECT COUNT(*) FROM request_logs WHERE app_id = $1", app).await),
                ("request_artifacts", self.count("SELECT COUNT(*) FROM request_artifacts WHERE app_id = $1", app).await),
                ("request_log_tags", self.count("SELECT COUNT(*) FROM request_log_tags WHERE tag = $1", tag).await),
            ]
        }
    }

    /// The mock vendor, which deletes app `app` when a 3D job or a
    /// segmentation is handed to it: the app is deleted while a request is
    /// with its vendor, before litegen writes its rows. One that `fails`
    /// then fails the request.
    struct DeletesTheAppAtTheVendor {
        db: Arc<dyn DatabaseStore>,
        app: String,
        fails: bool,
        image: crate::providers::image::mock::MockProvider,
        mesh: crate::providers::model3d::mock::MockModel3dProvider,
    }

    impl DeletesTheAppAtTheVendor {
        fn new(db: Arc<dyn DatabaseStore>, app: &str, fails: bool) -> Self {
            use crate::providers::{ImageProvider, Model3dProvider};
            let mut image = crate::providers::image::mock::MockProvider::new();
            ImageProvider::configure(&mut image, crate::providers::ProviderInstanceConfig::default());
            let mut mesh = crate::providers::model3d::mock::MockModel3dProvider::new();
            Model3dProvider::configure(&mut mesh, crate::providers::ProviderInstanceConfig::default());
            Self { db, app: app.to_string(), fails, image, mesh }
        }

        async fn delete_the_app(&self) -> Result<(), crate::providers::ProviderError> {
            assert!(self.db.delete_application(&self.app).await.unwrap(), "the app was there to delete");
            if !self.fails {
                return Ok(());
            }
            Err(crate::providers::ProviderError::RequestFailed { message: "the vendor is down".into(), status_code: Some(500), provider_error: None, retryable: false })
        }
    }

    #[async_trait::async_trait]
    impl crate::providers::ImageProvider for DeletesTheAppAtTheVendor {
        fn name(&self) -> &str {
            "mock"
        }
        fn configure(&mut self, _c: crate::providers::ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool {
            true
        }
        async fn generate(
            &self,
            m: &crate::capabilities::ModelSchema,
            b: &BaseGenerationRequest,
            e: &crate::providers::ImageExtras,
            mat: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<crate::providers::GenerationOutput, crate::providers::ProviderError> {
            self.delete_the_app().await?;
            crate::providers::ImageProvider::generate(&self.image, m, b, e, mat).await
        }
        async fn estimate_cost(&self, m: &crate::capabilities::ModelSchema, r: &ImageGenerationRequest) -> Result<CostEstimate, crate::providers::ProviderError> {
            crate::providers::ImageProvider::estimate_cost(&self.image, m, r).await
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            crate::providers::ImageProvider::health_check(&self.image).await
        }
        fn resumable(&self, m: &crate::capabilities::ModelSchema) -> bool {
            crate::providers::ImageProvider::resumable(&self.image, m)
        }
        async fn poll_job(&self, m: &str, job: &str, resume: Option<&serde_json::Value>) -> Result<crate::providers::ImageJobPoll, crate::providers::ProviderError> {
            crate::providers::ImageProvider::poll_job(&self.image, m, job, resume).await
        }
        async fn segment(&self, m: &crate::capabilities::ModelSchema, input: &crate::providers::SegmentationInput) -> Result<Vec<crate::providers::VendorMask>, crate::providers::ProviderError> {
            self.delete_the_app().await?;
            crate::providers::ImageProvider::segment(&self.image, m, input).await
        }
    }

    #[async_trait::async_trait]
    impl crate::providers::Model3dProvider for DeletesTheAppAtTheVendor {
        fn name(&self) -> &str {
            "mock"
        }
        fn configure(&mut self, _c: crate::providers::ProviderInstanceConfig) {}
        fn is_configured(&self) -> bool {
            true
        }
        async fn generate(
            &self,
            m: &crate::capabilities::ModelSchema,
            b: &BaseGenerationRequest,
            e: &crate::providers::Model3dExtras,
            mat: &crate::proxy::materializer::MaterializedRequest,
        ) -> Result<crate::providers::Model3dGenerationHandle, crate::providers::ProviderError> {
            self.delete_the_app().await?;
            crate::providers::Model3dProvider::generate(&self.mesh, m, b, e, mat).await
        }
        async fn poll_status(&self, h: &crate::providers::Model3dGenerationHandle) -> Result<crate::providers::Model3dGenerationPollResult, crate::providers::ProviderError> {
            crate::providers::Model3dProvider::poll_status(&self.mesh, h).await
        }
        async fn estimate_cost(&self, m: &crate::capabilities::ModelSchema, r: &Model3dGenerationRequest) -> Result<CostEstimate, crate::providers::ProviderError> {
            crate::providers::Model3dProvider::estimate_cost(&self.mesh, m, r).await
        }
        async fn health_check(&self) -> crate::providers::HealthCheckResult {
            crate::providers::Model3dProvider::health_check(&self.mesh).await
        }
    }

    /// Who sends a request: the tenant's API key, or a member signed in to
    /// the dashboard, whose request names no key (so no key's foreign key
    /// stops its generation row once the app is gone).
    #[derive(Clone, Copy)]
    enum Caller {
        Key,
        Dashboard,
    }

    /// A request: its route, its body, and its `Prefer` header if any.
    struct Call {
        uri: &'static str,
        body: serde_json::Value,
        prefer: Option<&'static str>,
    }

    /// A tenant's request whose app is deleted while it is with the vendor,
    /// which then fails it when it `fails`: nothing of the app's stays
    /// behind, its tags included.
    async fn a_request_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_on(rows: Rows, tag: &str, caller: Caller, fails: bool, call: Call) -> (StatusCode, serde_json::Value) {
        let db = rows.db();
        let (org, app, secret) = tenant_with_key(&*db, tag).await;
        let dir = tempfile::tempdir().unwrap();
        let state = state_over_with(db.clone(), Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into())), Some(MASTER_KEY), |_| {}).await;
        let vendor = Arc::new(DeletesTheAppAtTheVendor::new(db.clone(), &app, fails));
        state.router.registry.register_mock_image(vendor.clone()).await;
        state.router.registry.register_mock_model3d(vendor).await;
        let router = crate::api::handlers::create_router(state);
        let spend_tag = format!("spend-{tag}");
        let mut req = Request::post(call.uri).header("content-type", "application/json").header("x-litegen-tags", spend_tag.as_str());
        if let Some(prefer) = call.prefer {
            req = req.header("prefer", prefer);
        }
        let req = match caller {
            Caller::Key => req.header("authorization", format!("Bearer {secret}")),
            Caller::Dashboard => {
                let now = chrono::Utc::now();
                let user = crate::types::User {
                    id: format!("user-{tag}"),
                    email: format!("{tag}@example.com"),
                    password_hash: None,
                    role: crate::types::Role::Member,
                    oauth_github_id: None,
                    oauth_google_id: None,
                    created_at: now,
                    updated_at: now,
                    last_login_at: None,
                    is_active: true,
                    sign_in_epoch: 0,
                };
                db.create_user(&user).await.unwrap();
                db.add_org_member(&org, &user.id, crate::types::Role::Owner).await.unwrap();
                let (session, csrf, _, _) = crate::api::middleware::create_session_cookies(&db, &user.id, None, None).await.unwrap();
                req.header("cookie", format!("litegen_session={session}"))
                    .header("x-csrf-token", csrf)
                    .header("x-litegen-org-id", org.as_str())
                    .header("x-litegen-app-id", app.as_str())
            }
        };
        let req = req.body(Body::from(call.body.to_string())).unwrap();
        let resp = router.oneshot(req).await.unwrap();
        let status = resp.status();
        let answer: serde_json::Value = serde_json::from_slice(&axum::body::to_bytes(resp.into_body(), 64 << 20).await.unwrap()).unwrap_or(serde_json::Value::Null);
        let left = rows.left_of(&app, &spend_tag).await;
        assert!(left.iter().all(|(_, n)| *n == 0), "nothing of the deleted app's stays: {left:?} ({status} {answer})");
        (status, answer)
    }

    fn a_3d_submit() -> Call {
        Call { uri: "/v1/models3d/generations", body: serde_json::json!({ "model": "mock/mesh-3d", "prompt": "a cube" }), prefer: None }
    }

    fn a_segmentation() -> Call {
        let png = {
            let img = image::RgbaImage::from_fn(32, 24, |x, y| image::Rgba([(x * 5) as u8, (y * 7) as u8, 90, 255]));
            let mut out = Vec::new();
            img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).unwrap();
            base64::Engine::encode(&base64::engine::general_purpose::STANDARD, out)
        };
        let body = serde_json::json!({ "model": "mock/segment-image", "image": png, "prompts": [{ "type": "box", "x": 2, "y": 2, "w": 8, "h": 8 }] });
        Call { uri: "/v1/images/segmentations", body, prefer: None }
    }

    /// An image the vendor runs as a job, which its caller doesn't wait for:
    /// it is answered once the vendor took the job (a late image).
    fn a_late_image() -> Call {
        Call { uri: "/v1/images/generations", body: serde_json::json!({ "model": "mock/priced-image", "prompt": "a lighthouse job_ms=60000" }), prefer: Some("respond-async") }
    }

    /// A 3D job whose app is deleted at its vendor is let go there, and its
    /// submit fails as one whose row couldn't be written. From the dashboard
    /// its row is written all the same, then removed.
    async fn a_3d_submit_whose_app_is_deleted_at_the_vendor_on(rows: Rows, tag: &str, caller: Caller) {
        let (status, answer) = a_request_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_on(rows, tag, caller, false, a_3d_submit()).await;
        assert_eq!((status, answer["error"]["code"].clone()), (StatusCode::INTERNAL_SERVER_ERROR, serde_json::json!("generation_record_failed")), "{answer}");
    }

    /// A late image whose app is deleted at its vendor: its caller hears what
    /// that of a job litegen couldn't record does, a timeout naming the
    /// vendor's job. From the dashboard its row is written all the same, then
    /// removed.
    async fn a_late_image_whose_app_is_deleted_at_the_vendor_on(rows: Rows, tag: &str, caller: Caller) {
        let (status, answer) = a_request_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_on(rows, tag, caller, false, a_late_image()).await;
        assert_eq!((status, answer["error"]["code"].clone()), (StatusCode::GATEWAY_TIMEOUT, serde_json::json!("vendor_timeout")), "{answer}");
        assert!(answer["error"]["vendor_job_id"].as_str().is_some_and(|j| j.starts_with("mock-image-job-")), "{answer}");
    }

    #[tokio::test]
    async fn a_late_image_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        a_late_image_whose_app_is_deleted_at_the_vendor_on(Rows::Sqlite(db), "late-gone", Caller::Key).await;
    }

    #[tokio::test]
    async fn a_late_image_from_the_dashboard_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        a_late_image_whose_app_is_deleted_at_the_vendor_on(Rows::Sqlite(db), "board-late-gone", Caller::Dashboard).await;
    }

    #[tokio::test]
    async fn a_late_image_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping a_late_image_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        a_late_image_whose_app_is_deleted_at_the_vendor_on(Rows::Postgres(db), &format!("late-gone-{}", uuid::Uuid::new_v4().simple()), Caller::Key).await;
    }

    #[tokio::test]
    async fn a_late_image_from_the_dashboard_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping a_late_image_from_the_dashboard_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        a_late_image_whose_app_is_deleted_at_the_vendor_on(Rows::Postgres(db), &format!("board-late-gone-{}", uuid::Uuid::new_v4().simple()), Caller::Dashboard).await;
    }

    #[tokio::test]
    async fn a_3d_submit_from_the_dashboard_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        a_3d_submit_whose_app_is_deleted_at_the_vendor_on(Rows::Sqlite(db), "board-mesh-gone", Caller::Dashboard).await;
    }

    #[tokio::test]
    async fn a_3d_submit_from_the_dashboard_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping a_3d_submit_from_the_dashboard_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        a_3d_submit_whose_app_is_deleted_at_the_vendor_on(Rows::Postgres(db), &format!("board-mesh-gone-{}", uuid::Uuid::new_v4().simple()), Caller::Dashboard).await;
    }

    /// A request the vendor fails once its app is deleted: the vendor's
    /// answer reaches its caller, and its failed row goes with the app.
    async fn a_failed_request_whose_app_is_deleted_at_the_vendor_on(rows: Rows, tag: &str, call: Call) {
        let (status, answer) = a_request_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_on(rows, tag, Caller::Key, true, call).await;
        assert_eq!((status, answer["error"]["code"].clone()), (StatusCode::BAD_GATEWAY, serde_json::json!("vendor_error")), "{answer}");
    }

    #[tokio::test]
    async fn a_failed_3d_submit_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        a_failed_request_whose_app_is_deleted_at_the_vendor_on(Rows::Sqlite(db), "failed-mesh-gone", a_3d_submit()).await;
    }

    #[tokio::test]
    async fn a_failed_3d_submit_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping a_failed_3d_submit_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        a_failed_request_whose_app_is_deleted_at_the_vendor_on(Rows::Postgres(db), &format!("failed-mesh-gone-{}", uuid::Uuid::new_v4().simple()), a_3d_submit()).await;
    }

    #[tokio::test]
    async fn a_failed_segmentation_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        a_failed_request_whose_app_is_deleted_at_the_vendor_on(Rows::Sqlite(db), "failed-seg-gone", a_segmentation()).await;
    }

    #[tokio::test]
    async fn a_failed_segmentation_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping a_failed_segmentation_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        a_failed_request_whose_app_is_deleted_at_the_vendor_on(Rows::Postgres(db), &format!("failed-seg-gone-{}", uuid::Uuid::new_v4().simple()), a_segmentation()).await;
    }

    #[tokio::test]
    async fn a_3d_submit_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        a_3d_submit_whose_app_is_deleted_at_the_vendor_on(Rows::Sqlite(db), "mesh-gone", Caller::Key).await;
    }

    #[tokio::test]
    async fn a_3d_submit_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping a_3d_submit_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        a_3d_submit_whose_app_is_deleted_at_the_vendor_on(Rows::Postgres(db), &format!("mesh-gone-{}", uuid::Uuid::new_v4().simple()), Caller::Key).await;
    }

    /// A segmentation whose app is deleted at its vendor answers its caller,
    /// and leaves no log row (nor tags) of the deleted app's.
    #[tokio::test]
    async fn a_segmentation_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind() {
        let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let (status, answer) = a_request_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_on(Rows::Sqlite(db), "seg-gone", Caller::Key, false, a_segmentation()).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
    }

    #[tokio::test]
    async fn a_segmentation_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping a_segmentation_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        let (status, answer) = a_request_whose_app_is_deleted_at_the_vendor_leaves_nothing_behind_on(Rows::Postgres(db), &format!("seg-gone-{}", uuid::Uuid::new_v4().simple()), Caller::Key, false, a_segmentation()).await;
        assert_eq!(status, StatusCode::OK, "{answer}");
    }

    // ─── a cache hit's row, read back on Postgres ───────────────────────

    #[tokio::test]
    async fn a_cache_hits_row_says_so_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping a_cache_hits_row_says_so_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db: Arc<dyn DatabaseStore> = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        let (_, _, secret) = tenant_with_key(&*db, &format!("hits-{}", uuid::Uuid::new_v4().simple())).await;
        let dir = tempfile::tempdir().unwrap();
        let store: Arc<dyn ImageStorage> = Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()));
        let router = crate::api::handlers::create_router(state_over_with(db.clone(), store, Some(MASTER_KEY), |c| c.cache.enabled = true).await);
        let routes = ["/v1/logs?per_page=100", "/v1/spend/logs?per_page=100"];
        // The rows of this model already there (a shared database): not this test's.
        let mut before = std::collections::HashMap::new();
        for route in routes {
            let (status, _, page) = as_key(&router, "GET", route, &secret, None).await;
            assert_eq!(status, StatusCode::OK, "{route}: {page}");
            let ids: std::collections::HashSet<String> = page["data"].as_array().into_iter().flatten().filter_map(|r| r["id"].as_str().map(str::to_string)).collect();
            before.insert(route, ids);
        }
        let image = serde_json::json!({ "model": "mock/expensive-image", "prompt": "a red cube" });
        let (status, headers, first) = as_key(&router, "POST", "/v1/images/generations", &secret, Some(image.clone())).await;
        assert_eq!((status, headers.get("x-litegen-cache").map(|v| v.to_str().unwrap().to_string())), (StatusCode::OK, Some("miss".into())), "{first}");
        let (status, headers, second) = as_key(&router, "POST", "/v1/images/generations", &secret, Some(image)).await;
        assert_eq!((status, headers.get("x-litegen-cache").map(|v| v.to_str().unwrap().to_string())), (StatusCode::OK, Some("hit".into())), "{second}");

        for route in routes {
            let (status, _, page) = as_key(&router, "GET", route, &secret, None).await;
            assert_eq!(status, StatusCode::OK, "{route}: {page}");
            let rows: Vec<_> = page["data"]
                .as_array()
                .unwrap_or_else(|| panic!("{route}: {page}"))
                .iter()
                .filter(|r| r["model"] == "mock/expensive-image" && !before[route].contains(r["id"].as_str().unwrap_or_default()))
                .collect();
            assert_eq!(rows.len(), 2, "{route}: {rows:?}");
            for row in rows {
                let hit = row["id"] != first["id"];
                assert_eq!(row["cache_hit"], serde_json::Value::Bool(hit), "{route}: {row}");
            }
        }
    }

    // ─── what the check after a request's writes reads ──────────────────

    /// A database as it is, whose reads of an org or an app are counted.
    struct CountsTenantReads {
        inner: Arc<dyn DatabaseStore>,
        reads: std::sync::atomic::AtomicUsize,
    }

    #[async_trait::async_trait]
    impl DatabaseStore for CountsTenantReads {
        fn request_rows_reference_their_tenant(&self) -> bool {
            self.inner.request_rows_reference_their_tenant()
        }
        async fn get_application(&self, id: &str) -> Result<Option<Application>, sqlx::Error> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.get_application(id).await
        }
        async fn get_organization(&self, id: &str) -> Result<Option<Organization>, sqlx::Error> {
            self.reads.fetch_add(1, Ordering::SeqCst);
            self.inner.get_organization(id).await
        }
        async fn forget_request_rows(&self, generation_id: Option<&str>, request_id: &str) -> Result<(), sqlx::Error> {
            self.inner.forget_request_rows(generation_id, request_id).await
        }
        async fn record_tenant_file_deletions(&self, scope: &str, tenant_id: &str, generation_id: &str, urls: &[String]) -> Result<(), sqlx::Error> {
            self.inner.record_tenant_file_deletions(scope, tenant_id, generation_id, urls).await
        }
        async fn log_request(
            &self, id: &str, model: &str, provider: &str, status: &str, media_type: &str, cost_usd: f64, latency_ms: i64,
            error: Option<&str>, metadata: Option<&serde_json::Value>, org_id: Option<&str>, app_id: Option<&str>,
        ) -> Result<(), sqlx::Error> {
            self.inner.log_request(id, model, provider, status, media_type, cost_usd, latency_ms, error, metadata, org_id, app_id).await
        }
        async fn insert_generation(
            &self, id: &str, key_id: Option<&uuid::Uuid>, model: &str, provider: &str, media_type: &str,
            provider_job_id: Option<&str>, cost_usd: f64, org_id: Option<&str>, app_id: Option<&str>,
        ) -> Result<(), sqlx::Error> {
            self.inner.insert_generation(id, key_id, model, provider, media_type, provider_job_id, cost_usd, org_id, app_id).await
        }
        async fn update_generation_status(
            &self, id: &str, status: &str, progress: i32, result_url: Option<&str>, error: Option<&str>,
            completed_at: Option<chrono::DateTime<chrono::Utc>>,
        ) -> Result<(), sqlx::Error> {
            self.inner.update_generation_status(id, status, progress, result_url, error, completed_at).await
        }
        async fn update_generation_metadata(&self, id: &str, metadata: &serde_json::Value) -> Result<(), sqlx::Error> {
            self.inner.update_generation_metadata(id, metadata).await
        }
        async fn update_request_log_status(&self, id: &str, status: &str, error: Option<&str>) -> Result<(), sqlx::Error> {
            self.inner.update_request_log_status(id, status, error).await
        }
        async fn get_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error> {
            self.inner.get_generation(id).await
        }
        async fn list_active_generations(&self, limit: u32) -> Result<Vec<Generation>, sqlx::Error> {
            self.inner.list_active_generations(limit).await
        }
        async fn list_generations(&self, key_id: Option<&uuid::Uuid>, page: u32, per_page: u32) -> Result<Vec<Generation>, sqlx::Error> {
            self.inner.list_generations(key_id, page, per_page).await
        }
        async fn count_generations(&self, key_id: Option<&uuid::Uuid>) -> Result<i64, sqlx::Error> {
            self.inner.count_generations(key_id).await
        }
        async fn cancel_generation(&self, id: &str) -> Result<Option<Generation>, sqlx::Error> {
            self.inner.cancel_generation(id).await
        }
        async fn get_request_logs(&self, page: u32, per_page: u32) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
            self.inner.get_request_logs(page, per_page).await
        }
        async fn get_request_logs_filtered(&self, filters: &LogFilters, page: u32, per_page: u32) -> Result<(Vec<RequestLog>, u64), sqlx::Error> {
            self.inner.get_request_logs_filtered(filters, page, per_page).await
        }
        async fn create_api_key(
            &self, name: &str, key_hash: &str, key_prefix: &str, token_quota: Option<f64>, rpm_limit: Option<u32>, scopes: &str,
            webhook_url: Option<&str>,
        ) -> Result<ApiKey, sqlx::Error> {
            self.inner.create_api_key(name, key_hash, key_prefix, token_quota, rpm_limit, scopes, webhook_url).await
        }
        async fn get_api_key(&self, id: &uuid::Uuid) -> Result<Option<ApiKey>, sqlx::Error> {
            self.inner.get_api_key(id).await
        }
        async fn update_api_key(&self, id: &uuid::Uuid, req: &UpdateApiKeyRequest) -> Result<Option<ApiKey>, sqlx::Error> {
            self.inner.update_api_key(id, req).await
        }
        async fn lookup_api_key_by_hash(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
            self.inner.lookup_api_key_by_hash(key_hash).await
        }
        async fn atomic_charge_tokens(&self, id: &uuid::Uuid, cost_usd: f64) -> Result<f64, sqlx::Error> {
            self.inner.atomic_charge_tokens(id, cost_usd).await
        }
        async fn validate_api_key(&self, key_hash: &str) -> Result<Option<ApiKey>, sqlx::Error> {
            self.inner.validate_api_key(key_hash).await
        }
        async fn list_api_keys(&self) -> Result<Vec<ApiKey>, sqlx::Error> {
            self.inner.list_api_keys().await
        }
        async fn revoke_api_key(&self, id: &uuid::Uuid) -> Result<bool, sqlx::Error> {
            self.inner.revoke_api_key(id).await
        }
        async fn get_stats(&self) -> Result<ProxyStats, sqlx::Error> {
            self.inner.get_stats().await
        }
    }

    /// A member's context in `org`'s `app`.
    fn member_of(org: &str, app: &str) -> KeyContext {
        KeyContext { org_id: Some(org.to_string()), app_id: Some(app.to_string()), ..platform_admin() }
    }

    /// How many times the check after a request's writes reads the org or
    /// the app, for a request of a standing app that made a generation or not.
    async fn tenant_reads(db: Arc<dyn DatabaseStore>, tag: &str, made_a_generation: bool) -> usize {
        let (org, app) = tenant(&*db, tag).await;
        let counting = CountsTenantReads { inner: db, reads: Default::default() };
        let request = format!("req-{tag}");
        let generation = made_a_generation.then_some(request.as_str());
        let gone = super::forget_a_request_of_a_vanished_tenant(&counting, Some(&member_of(&org, &app)), &request, generation, &[]).await;
        assert!(!gone, "the app stands");
        counting.reads.load(Ordering::SeqCst)
    }

    /// On SQLite nothing ties a request's rows to its tenant: the check reads
    /// the app, whatever the request made.
    #[tokio::test]
    async fn the_check_after_a_request_reads_its_app_on_sqlite() {
        for (tag, made) in [("reads-none", false), ("reads-made", true)] {
            let db = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
            assert_eq!(tenant_reads(db, tag, made).await, 1, "{tag}");
        }
    }

    /// On Postgres keys tie every row a request writes to its tenant: the
    /// check of a request that made no generation reads nothing, and that of
    /// one that made a generation still reads the app.
    #[tokio::test]
    async fn the_check_after_a_request_reads_only_for_a_generation_against_postgres() {
        let Ok(url) = std::env::var("LITEGEN_PG_TEST_URL") else {
            eprintln!("skipping the_check_after_a_request_reads_only_for_a_generation_against_postgres: set LITEGEN_PG_TEST_URL");
            return;
        };
        let db: Arc<dyn DatabaseStore> = Arc::new(crate::db::PostgresDatabase::connect(&url).await.expect("connect + migrate against the test Postgres"));
        let run = uuid::Uuid::new_v4().simple().to_string();
        assert_eq!(tenant_reads(db.clone(), &format!("reads-none-{run}"), false).await, 0, "no generation: no read");
        assert_eq!(tenant_reads(db, &format!("reads-made-{run}"), true).await, 1, "a generation: one read");
    }

    // ─── a delete whose caller hangs up ─────────────────────────────────

    #[tokio::test]
    async fn an_app_is_deleted_all_the_same_when_its_caller_hangs_up() {
        let inner = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let (_, app) = tenant(&*inner, "hangs-up").await;
        let held = Arc::new(Held::new(inner.clone()));
        let dir = tempfile::tempdir().unwrap();
        let state = state_over(held.clone(), Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()))).await;

        let mut request = Box::pin(crate::api::handlers::orgs::delete_app(State(state), Extension(platform_admin()), Path(app.clone())));
        tokio::select! {
            _ = &mut request => panic!("it answered before its delete began"),
            _ = held.entered.notified() => {}
        }
        drop(request);
        held.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), held.done.notified()).await.expect("the delete went on after its caller hung up");
        assert!(inner.get_application(&app).await.unwrap().is_none(), "and committed");
    }

    #[tokio::test]
    async fn an_org_is_deleted_all_the_same_when_its_caller_hangs_up() {
        let inner = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let (org, _) = tenant(&*inner, "org-hangs-up").await;
        let held = Arc::new(Held::new(inner.clone()));
        let dir = tempfile::tempdir().unwrap();
        let state = state_over(held.clone(), Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()))).await;

        let mut request = Box::pin(crate::api::handlers::orgs::delete_org(State(state), Extension(platform_admin()), Path(org.clone())));
        tokio::select! {
            _ = &mut request => panic!("it answered before its delete began"),
            _ = held.entered.notified() => {}
        }
        drop(request);
        held.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), held.done.notified()).await.expect("the delete went on after its caller hung up");
        assert!(inner.get_organization(&org).await.unwrap().is_none(), "and committed");
    }

    // ─── a failed request's log row ─────────────────────────────────────

    #[tokio::test]
    async fn a_failed_generations_log_row_is_written_when_its_request_is_dropped() {
        let inner = Arc::new(SqliteDatabase::connect("sqlite::memory:").await.unwrap());
        let held = Arc::new(Held::new(inner.clone()));
        let dir = tempfile::tempdir().unwrap();
        let state = state_over(held.clone(), Arc::new(LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000".into()))).await;
        let schema = state.registry.get("mock/image-gen").expect("the mock image model").clone();
        let logging = async move {
            let error = crate::proxy::router::ProxyError::Internal("the vendor is down".into());
            let attributed = crate::management::types::Attribution::default();
            crate::api::handlers::log_failed_generation(&state, None, crate::api::handlers::FailedGeneration {
                id_prefix: "litegen-img",
                media_type: "image",
                schema: &schema,
                latency_ms: 5,
                error: &error,
                prompt: "a fox",
                metadata: None,
                attributed: &attributed,
            })
            .await
        };
        let mut request = Box::pin(logging);
        tokio::select! {
            _ = &mut request => panic!("it finished before its write began"),
            _ = held.entered.notified() => {}
        }
        // The request is dropped mid-write: a stop's cut, at the end of its grace.
        drop(request);
        held.release.notify_one();
        tokio::time::timeout(Duration::from_secs(10), held.done.notified()).await.expect("the failed row's write went on after its request was dropped");
        let (rows, _) = inner.get_request_logs(1, 10).await.unwrap();
        assert_eq!(rows.len(), 1, "one failed row");
    }
}
