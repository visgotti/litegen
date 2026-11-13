//! Signed links to files in litegen's own store.
//!
//! An S3 bucket signs its own links (a presigned GET). litegen's local store
//! signs with an HMAC over the file's key and the link's expiry, which
//! `GET /v1/models3d/assets/{key}` checks before serving.

use base64::Engine;
use hmac::{Hmac, Mac};
use sha2::Sha256;

use crate::db::DatabaseStore;

type HmacSha256 = Hmac<Sha256>;

/// Signs and checks links to litegen's own stored files.
pub struct LinkSigner {
    key: [u8; 32],
    persistent: bool,
}

impl std::fmt::Debug for LinkSigner {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("LinkSigner").field("persistent", &self.persistent).finish_non_exhaustive()
    }
}

impl LinkSigner {
    /// A signer keyed from `LITEGEN__SECRETS_KEY` (base64, 32 bytes), so links
    /// outlive a restart and verify on every replica. Without one, a random
    /// key: links stop working when this process does.
    pub fn from_secrets_key(secrets_key: Option<&str>) -> Self {
        match secrets_key.and_then(decode_key) {
            Some(secret) => {
                let mut mac = HmacSha256::new_from_slice(&secret).expect("HMAC takes a key of any length");
                mac.update(b"litegen-asset-links-v1");
                Self { key: mac.finalize().into_bytes().into(), persistent: true }
            }
            None => Self { key: rand::random(), persistent: false },
        }
    }

    /// A signer keyed from the key `link_signer_for` stored in the database:
    /// base64 for exactly 32 bytes, used as the key itself (it is already a
    /// dedicated random key, so nothing is derived from it). `None` for
    /// anything else.
    pub fn from_stored_key(stored_b64: &str) -> Option<Self> {
        decode_key(stored_b64).map(|key| Self { key, persistent: true })
    }

    /// Whether links survive a restart (the key came from the secrets key or
    /// the database, not from this process's own random draw).
    pub fn persistent(&self) -> bool {
        self.persistent
    }

    fn mac(&self, key: &str, expires: i64) -> HmacSha256 {
        let mut mac = HmacSha256::new_from_slice(&self.key).expect("HMAC takes a key of any length");
        mac.update(expires.to_string().as_bytes());
        mac.update(b"\n");
        mac.update(key.as_bytes());
        mac
    }

    /// The signature of a link to `key` that expires at `expires` (unix seconds), as hex.
    pub fn sign(&self, key: &str, expires: i64) -> String {
        hex::encode(self.mac(key, expires).finalize().into_bytes())
    }

    /// Whether `signature` was made by this signer for exactly this key and
    /// expiry. Constant-time. Says nothing about whether it has expired.
    pub fn verify(&self, key: &str, expires: i64, signature: &str) -> bool {
        let Ok(sig) = hex::decode(signature) else { return false };
        self.mac(key, expires).verify_slice(&sig).is_ok()
    }

    /// The query string a signed link carries.
    pub fn query(&self, key: &str, expires: i64) -> String {
        format!("expires={expires}&signature={}", self.sign(key, expires))
    }

    /// What a local-store link issued at `issued` signs: its key and the issue
    /// time, NUL-separated so no key can stand for another.
    fn local_key(key: &str, issued: i64) -> String {
        format!("{key}\0issued={issued}")
    }

    /// The query string of a local-store link to `key`, issued at `issued`
    /// (unix milliseconds) and expiring at `expires` (unix seconds).
    pub fn local_query(&self, key: &str, issued: i64, expires: i64) -> String {
        format!("expires={expires}&issued={issued}&signature={}", self.sign(&Self::local_key(key, issued), expires))
    }

    /// Whether a local-store link verifies. A link without `issued` was minted
    /// before links carried one, and verifies the old way.
    pub fn verify_local(&self, key: &str, issued: Option<i64>, expires: i64, signature: &str) -> bool {
        match issued {
            Some(issued) => self.verify(&Self::local_key(key, issued), expires, signature),
            None => self.verify(key, expires, signature),
        }
    }

    /// What a link to litegen's signed file route signs: the generation, the
    /// file's index and when the link was issued. The NUL bytes keep it apart
    /// from every local-store key, which is a URL path.
    fn file_route_key(generation_id: &str, index: usize, issued: i64) -> String {
        format!("\0generation-file\0{generation_id}/{index}@{issued}")
    }

    /// The query string of a link to `GET /v1/generations/{id}/assets/{index}/signed`,
    /// issued at `issued` and expiring at `expires` (unix seconds).
    pub fn file_route_query(&self, generation_id: &str, index: usize, issued: i64, expires: i64) -> String {
        let signature = self.sign(&Self::file_route_key(generation_id, index, issued), expires);
        format!("expires={expires}&issued={issued}&signature={signature}")
    }

    /// Whether `signature` was made by this signer for exactly this file,
    /// issue time and expiry. Says nothing about expiry or revocation.
    pub fn verify_file_route(&self, generation_id: &str, index: usize, issued: i64, expires: i64, signature: &str) -> bool {
        self.verify(&Self::file_route_key(generation_id, index, issued), expires, signature)
    }
}

/// The signer this process uses. A usable `LITEGEN__SECRETS_KEY` wins. Without
/// one, the key stored in `server_secrets` (made on first use), which every
/// replica reads back, so links verify on all of them and survive a restart.
/// A database error is tried again for about 8 s ([`BOOT_RETRY_WAITS`]), so a
/// blip at boot is ridden out. If the database still can't supply the key, or
/// the stored value is not a key, this gives a random key for this process
/// (`persistent()` false), as `from_secrets_key(None)` does, and `main`
/// refuses to start on it.
pub async fn link_signer_for(secrets_key: Option<&str>, db: &dyn DatabaseStore) -> LinkSigner {
    link_signer_retrying(secrets_key, db, BOOT_RETRY_WAITS).await
}

/// The waits before each further try at the stored key, about 8 s in all.
const BOOT_RETRY_WAITS: &[std::time::Duration] = &[
    std::time::Duration::from_millis(250),
    std::time::Duration::from_millis(500),
    std::time::Duration::from_secs(1),
    std::time::Duration::from_secs(2),
    std::time::Duration::from_secs(4),
];

/// [`link_signer_for`], trying the database again after each of `waits` when
/// it answers with an error. A stored value that is not a key is not retried.
async fn link_signer_retrying(
    secrets_key: Option<&str>,
    db: &dyn DatabaseStore,
    waits: &[std::time::Duration],
) -> LinkSigner {
    // A blank value is an unset one (an empty environment variable).
    if let Some(raw) = secrets_key.filter(|s| !s.trim().is_empty()) {
        if decode_key(raw).is_some() {
            return LinkSigner::from_secrets_key(Some(raw));
        }
        // Never the value: it is a secret, and a near miss is still close to one.
        tracing::warn!(
            "LITEGEN__SECRETS_KEY is set but is not base64 for exactly 32 bytes, so it is ignored for signed links: \
             using the link-signing key stored in the database"
        );
    }
    let generate = || base64::engine::general_purpose::STANDARD.encode(rand::random::<[u8; 32]>());
    let mut waits = waits.iter();
    loop {
        match db.server_secret(LINK_SIGNING_KEY, &generate).await {
            Ok(stored) => {
                return LinkSigner::from_stored_key(&stored).unwrap_or_else(|| {
                    // Never the value: it may be a damaged key.
                    tracing::error!(
                        "the link-signing key stored in the database is not base64 for 32 bytes \
                         (server_secrets, name link_signing_key)"
                    );
                    LinkSigner::from_secrets_key(None)
                })
            }
            Err(e) => match waits.next() {
                Some(wait) => {
                    tracing::warn!(
                        error = %e,
                        "could not read or store the link-signing key in the database: trying again in {} ms",
                        wait.as_millis()
                    );
                    tokio::time::sleep(*wait).await;
                }
                None => {
                    tracing::error!(error = %e, "could not read or store the link-signing key in the database");
                    return LinkSigner::from_secrets_key(None);
                }
            },
        }
    }
}

/// What the link-signing key is called in `server_secrets`.
const LINK_SIGNING_KEY: &str = "link_signing_key";

/// 32 bytes from `b64` (standard base64, surrounding whitespace ignored), or
/// `None` for anything else.
fn decode_key(b64: &str) -> Option<[u8; 32]> {
    base64::engine::general_purpose::STANDARD.decode(b64.trim()).ok()?.try_into().ok()
}

/// The engine's stores sign their own links through this (engine boundary,
/// D2); the secret stays here.
impl crate::store::LinkSigning for LinkSigner {
    fn local_query(&self, key: &str, issued: i64, expires: i64) -> String {
        LinkSigner::local_query(self, key, issued, expires)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::SqliteDatabase;
    use base64::Engine;
    use std::sync::{Arc, Mutex};

    fn secrets_key(byte: u8) -> String {
        base64::engine::general_purpose::STANDARD.encode([byte; 32])
    }

    // ─── The stored key ────────────────────────────────────────────────

    /// A connection to the SQLite file in `dir`: one replica. Several of them
    /// over one file are several replicas over one database.
    async fn open(dir: &tempfile::TempDir) -> SqliteDatabase {
        let url = format!("sqlite://{}?mode=rwc", dir.path().join("litegen.db").display());
        SqliteDatabase::connect(&url).await.expect("a SQLite file")
    }

    /// What the log lines written while the guard lives said.
    #[derive(Clone, Default)]
    struct Logs(Arc<Mutex<Vec<u8>>>);

    impl Logs {
        fn text(&self) -> String {
            String::from_utf8_lossy(&self.0.lock().unwrap()).into_owned()
        }
    }

    impl std::io::Write for Logs {
        fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }
        fn flush(&mut self) -> std::io::Result<()> {
            Ok(())
        }
    }

    impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for Logs {
        type Writer = Logs;
        fn make_writer(&'a self) -> Logs {
            self.clone()
        }
    }

    /// Every level, captured on this thread (a `#[tokio::test]` runs on one),
    /// with the process-wide subscriber held first so no callsite this test
    /// wants is off (`util::test_logs`, FU-SPANFLAKE).
    fn capture_logs() -> (Logs, tracing::subscriber::DefaultGuard) {
        crate::util::test_logs::hold_process_subscriber();
        let logs = Logs::default();
        let subscriber = tracing_subscriber::fmt().with_writer(logs.clone()).with_ansi(false).with_max_level(tracing::Level::TRACE).finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    // What sqlx logged about its statements while it lives, through the
    // process-wide subscriber: sqlx's SQLite driver logs from a worker thread
    // of its own, which the per-thread subscriber of `capture_logs` never
    // reaches.
    use crate::util::test_logs::SqlxLogs;

    #[tokio::test]
    async fn two_signers_over_one_database_agree() {
        let dir = tempfile::tempdir().unwrap();
        let (db1, db2) = (open(&dir).await, open(&dir).await);
        let (a, b) = (link_signer_for(None, &db1).await, link_signer_for(None, &db2).await);
        assert!(a.persistent() && b.persistent());
        assert!(b.verify("k", 99, &a.sign("k", 99)), "a link signed on one replica verifies on the other");
        assert!(a.verify("k", 99, &b.sign("k", 99)));
    }

    #[tokio::test]
    async fn the_secrets_key_still_wins() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir).await;
        let key = secrets_key(7);
        let signer = link_signer_for(Some(&key), &db).await;
        assert_eq!(signer.sign("k", 1), LinkSigner::from_secrets_key(Some(&key)).sign("k", 1));
        assert!(signer.persistent());
        let stored: Option<String> = sqlx::query_scalar("SELECT value FROM server_secrets WHERE name = 'link_signing_key'").fetch_optional(db.pool()).await.unwrap();
        assert_eq!(stored, None, "a secrets key makes no stored one");
    }

    #[tokio::test]
    async fn a_restart_keeps_the_key() {
        let dir = tempfile::tempdir().unwrap();
        let s1 = link_signer_for(None, &open(&dir).await).await.sign("k", 5);
        let s2 = link_signer_for(None, &open(&dir).await).await;
        assert_eq!(s2.sign("k", 5), s1);
        assert!(s2.persistent());
    }

    #[tokio::test]
    async fn a_secrets_key_that_cannot_be_used_is_named_and_the_stored_key_serves() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir).await;
        let stored = link_signer_for(None, &db).await;
        for bad in ["hunter2-not-base64!", &base64::engine::general_purpose::STANDARD.encode([1u8; 16])] {
            let (logs, _guard) = capture_logs();
            let signer = link_signer_for(Some(bad), &db).await;
            let text = logs.text();
            assert!(signer.persistent());
            assert_eq!(signer.sign("k", 1), stored.sign("k", 1), "the stored key serves");
            assert!(text.contains("WARN") && text.contains("LITEGEN__SECRETS_KEY"), "a warning names the variable: {text}");
            assert!(!text.contains(bad) && !text.contains("hunter2"), "and never its value: {text}");
        }
        // Blank is as good as unset: no warning.
        let (logs, _guard) = capture_logs();
        assert_eq!(link_signer_for(Some("  "), &db).await.sign("k", 1), stored.sign("k", 1));
        assert!(!logs.text().contains("WARN"), "{}", logs.text());
    }

    #[tokio::test]
    async fn a_stored_value_that_is_not_a_key_falls_back_without_printing_it() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir).await;
        for tampered in ["hunter2-tampered-value".to_string(), base64::engine::general_purpose::STANDARD.encode([3u8; 31])] {
            sqlx::query("DELETE FROM server_secrets").execute(db.pool()).await.unwrap();
            sqlx::query("INSERT INTO server_secrets (name, value) VALUES ('link_signing_key', ?)").bind(&tampered).execute(db.pool()).await.unwrap();
            let (logs, _guard) = capture_logs();
            let signer = link_signer_for(None, &db).await;
            let text = logs.text();
            assert!(!signer.persistent(), "today's random key: links stop with this process");
            assert!(text.contains("ERROR") && text.contains("link-signing key"), "an error says so: {text}");
            assert!(!text.contains(&tampered) && !text.contains("hunter2"), "and never the stored value: {text}");
        }
    }

    #[tokio::test]
    async fn a_database_that_fails_falls_back_to_a_random_key() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir).await;
        sqlx::query("DROP TABLE server_secrets").execute(db.pool()).await.unwrap();
        let (logs, _guard) = capture_logs();
        // No waits: `link_signer_for` would try for about 8 s each time.
        let (a, b) = (link_signer_retrying(None, &db, &[]).await, link_signer_retrying(None, &db, &[]).await);
        assert!(!a.persistent() && !b.persistent());
        assert_ne!(a.sign("k", 1), b.sign("k", 1), "a random key each time");
        assert!(logs.text().contains("ERROR") && logs.text().contains("link-signing key"), "{}", logs.text());
    }

    /// A database that fails one query at boot (a blip) is asked again, and
    /// the replica signs with the key every replica shares, not one of its own.
    #[tokio::test]
    async fn a_database_blip_at_boot_is_tried_again_and_the_stored_key_serves() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir).await;
        let stored = link_signer_for(None, &db).await; // the key every replica uses
        // Exactly the next insert into server_secrets fails; the ones after it
        // go through. The insert runs even when the key is there (`ON CONFLICT
        // DO NOTHING`), and RAISE(FAIL) keeps the trigger's own update.
        for statement in [
            "CREATE TABLE blip (n INTEGER NOT NULL)",
            "INSERT INTO blip VALUES (0)",
            "CREATE TRIGGER fail_once BEFORE INSERT ON server_secrets WHEN (SELECT n FROM blip) = 0 \
             BEGIN UPDATE blip SET n = 1; SELECT RAISE(FAIL, 'a database blip'); END",
        ] {
            sqlx::query(statement).execute(db.pool()).await.unwrap();
        }
        let signer = link_signer_for(None, &db).await;
        let blips: i64 = sqlx::query_scalar("SELECT n FROM blip").fetch_one(db.pool()).await.unwrap();
        assert_eq!(blips, 1, "the first try met the blip");
        assert!(signer.persistent(), "one failed read at boot left this replica on a random key of its own");
        assert_eq!(signer.sign("k", 1), stored.sign("k", 1), "the key every replica shares");
    }

    #[tokio::test]
    async fn the_stored_key_never_reaches_a_log_or_debug() {
        let dir = tempfile::tempdir().unwrap();
        let db = open(&dir).await;
        let (logs, _guard) = capture_logs();
        let sqlx_logs = SqlxLogs::open();
        // A capture that saw nothing would find no key in any log. This
        // statement is a probe: its text must show up in what sqlx logged, or
        // the capture is not seeing sqlx's events and proves nothing.
        let probe = format!("capture-probe-{}", uuid::Uuid::new_v4());
        let probed: String = sqlx::query_scalar(&format!("SELECT '{probe}'")).fetch_one(db.pool()).await.unwrap();
        assert_eq!(probed, probe);
        assert!(sqlx_logs.text().contains(&probe), "sqlx's statements are not captured: {:?}", sqlx_logs.text());

        let signer = link_signer_for(None, &db).await;
        let again = link_signer_for(None, &db).await;
        let stored: String = sqlx::query_scalar("SELECT value FROM server_secrets WHERE name = 'link_signing_key'").fetch_one(db.pool()).await.unwrap();
        let raw = base64::engine::general_purpose::STANDARD.decode(&stored).unwrap();
        assert_eq!(raw.len(), 32);
        let everything = format!("{} {} {signer:?} {again:?}", logs.text(), sqlx_logs.text());
        assert!(!everything.contains(&stored), "base64 form: {everything}");
        assert!(!everything.contains(&hex::encode(&raw)), "hex form: {everything}");
    }

    #[test]
    fn a_stored_key_is_the_signing_key_itself() {
        let raw = [9u8; 32];
        let signer = LinkSigner::from_stored_key(&base64::engine::general_purpose::STANDARD.encode(raw)).expect("32 bytes of base64");
        assert!(signer.persistent());
        let mut mac = HmacSha256::new_from_slice(&raw).unwrap();
        mac.update(b"5\nk");
        assert_eq!(signer.sign("k", 5), hex::encode(mac.finalize().into_bytes()), "no derivation: the bytes are the key");
        assert_ne!(signer.sign("k", 5), LinkSigner::from_secrets_key(Some(&secrets_key(9))).sign("k", 5), "a secrets key is derived from; a stored key is not");
        for bad in [
            String::new(),
            "not base64!".to_string(),
            base64::engine::general_purpose::STANDARD.encode([9u8; 31]),
            base64::engine::general_purpose::STANDARD.encode([9u8; 33]),
        ] {
            assert!(LinkSigner::from_stored_key(&bad).is_none(), "{bad:?}");
        }
    }

    #[test]
    fn a_link_verifies_only_for_its_key_and_expiry() {
        let signer = LinkSigner::from_secrets_key(Some(&secrets_key(7)));
        let key = "litegen/3d/litegen-3d-abc/model.glb";
        let sig = signer.sign(key, 1_900_000_000);
        assert!(signer.verify(key, 1_900_000_000, &sig));
        assert!(!signer.verify("litegen/3d/litegen-3d-abc/model.fbx", 1_900_000_000, &sig), "another file");
        assert!(!signer.verify("litegen/3d/litegen-3d-abd/model.glb", 1_900_000_000, &sig), "another generation");
        assert!(!signer.verify(key, 1_900_000_001, &sig), "a later expiry");
        assert!(!signer.verify(key, 1_900_000_000, &sig[..sig.len() - 2]), "a truncated signature");
        assert!(!signer.verify(key, 1_900_000_000, ""), "no signature");
        let mut flipped = sig.clone().into_bytes();
        flipped[0] = if flipped[0] == b'0' { b'1' } else { b'0' };
        assert!(!signer.verify(key, 1_900_000_000, &String::from_utf8(flipped).unwrap()), "a changed signature");
        assert!(signer.verify(key, 1_900_000_000, &sig.to_ascii_uppercase()), "hex in either case");
        assert!(!signer.verify(key, 1_900_000_000, "zz"), "not hex");
    }

    #[test]
    fn a_file_route_link_verifies_only_for_its_generation_index_and_times() {
        let signer = LinkSigner::from_secrets_key(Some(&secrets_key(9)));
        let q = signer.file_route_query("litegen-vid-1", 0, 1_800_000_000, 1_900_000_000);
        let sig = q.rsplit_once("signature=").unwrap().1;
        assert!(q.starts_with("expires=1900000000&issued=1800000000&signature="), "{q}");
        assert!(signer.verify_file_route("litegen-vid-1", 0, 1_800_000_000, 1_900_000_000, sig));
        assert!(!signer.verify_file_route("litegen-vid-2", 0, 1_800_000_000, 1_900_000_000, sig), "another generation");
        assert!(!signer.verify_file_route("litegen-vid-1", 1, 1_800_000_000, 1_900_000_000, sig), "another file");
        assert!(!signer.verify_file_route("litegen-vid-1", 0, 1_800_000_001, 1_900_000_000, sig), "another issue time");
        assert!(!signer.verify_file_route("litegen-vid-1", 0, 1_800_000_000, 1_900_000_001, sig), "another expiry");
        // Never a local-store link, nor the other way round.
        assert!(!signer.verify("litegen-vid-1/0@1800000000", 1_900_000_000, sig));
        let local = signer.sign("litegen/video/litegen-vid-1/video.mp4", 1_900_000_000);
        assert!(!signer.verify_file_route("litegen-vid-1", 0, 1_800_000_000, 1_900_000_000, &local));
    }

    #[test]
    fn the_secrets_key_makes_links_survive_a_restart() {
        let key = "litegen/video/litegen-vid-1/video.mp4";
        let a = LinkSigner::from_secrets_key(Some(&secrets_key(1)));
        let b = LinkSigner::from_secrets_key(Some(&secrets_key(1)));
        assert_eq!(a.sign(key, 5), b.sign(key, 5), "same secrets key, same links: another replica or a restart verifies them");
        let c = LinkSigner::from_secrets_key(Some(&secrets_key(2)));
        assert_ne!(a.sign(key, 5), c.sign(key, 5));
        // No (or an unusable) secrets key: a random key per process.
        let d = LinkSigner::from_secrets_key(None);
        let e = LinkSigner::from_secrets_key(Some("not base64!"));
        assert_ne!(d.sign(key, 5), e.sign(key, 5));
        assert!(!d.persistent() && !e.persistent() && a.persistent());
    }

    #[test]
    fn a_link_query_carries_the_expiry_and_its_signature() {
        let signer = LinkSigner::from_secrets_key(Some(&secrets_key(3)));
        let q = signer.query("litegen/3d/g/model.glb", 1_900_000_000);
        assert_eq!(q, format!("expires=1900000000&signature={}", signer.sign("litegen/3d/g/model.glb", 1_900_000_000)));
    }
}
