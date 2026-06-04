//! Direct DB-layer tests for per-app BYO storage config. Verifies upsert/get/delete
//! and that the credential pair is encrypted at rest (plaintext never stored).
use std::sync::Arc;

use litegen::auth::secrets;
use litegen::db::sqlite::SqliteDatabase;
use litegen::db::DatabaseStore;
use litegen::types::AppStorageUpsert;

// The 0008 backfill always inserts this default application row, satisfying the
// app_storage_credentials.app_id FK without creating an org/app by hand.
const DEFAULT_APP_ID: &str = "00000000-0000-0000-0000-000000000002";
const KEY: [u8; 32] = [9u8; 32];

async fn db() -> (Arc<dyn DatabaseStore>, tempfile::NamedTempFile) {
    let tmp = tempfile::NamedTempFile::new().expect("tempfile");
    let url = format!("sqlite://{}?mode=rwc", tmp.path().display());
    let db: Arc<dyn DatabaseStore> =
        Arc::new(SqliteDatabase::connect(&url).await.expect("connect + migrate"));
    (db, tmp)
}

fn encrypt_keys(access: &str, secret: &str) -> (String, String) {
    let plaintext = serde_json::to_vec(
        &serde_json::json!({ "access_key_id": access, "secret_access_key": secret }),
    )
    .unwrap();
    secrets::encrypt(&KEY, &plaintext).unwrap()
}

#[tokio::test]
async fn app_storage_upsert_get_delete_roundtrip() {
    let (db, _tmp) = db().await;

    assert!(db.get_app_storage(DEFAULT_APP_ID).await.unwrap().is_none());

    let (ct, nonce) = encrypt_keys("AKIAEXAMPLE123", "s3cr3t-value");
    let input = AppStorageUpsert {
        app_id: DEFAULT_APP_ID.to_string(),
        backend: "s3".to_string(),
        bucket_name: "my-bucket".to_string(),
        region: "us-west-2".to_string(),
        endpoint_url: Some("https://minio.example.com".to_string()),
        custom_public_url: None,
        path_prefix: Some("litegen/images".to_string()),
        access_key_id_hint: Some("…E123".to_string()),
        secret_ciphertext: ct.clone(),
        secret_nonce: nonce.clone(),
    };
    db.upsert_app_storage(&input).await.unwrap();

    let row = db.get_app_storage(DEFAULT_APP_ID).await.unwrap().expect("row present");
    assert_eq!(row.backend, "s3");
    assert_eq!(row.bucket_name, "my-bucket");
    assert_eq!(row.region, "us-west-2");
    assert_eq!(row.endpoint_url.as_deref(), Some("https://minio.example.com"));
    assert_eq!(row.access_key_id_hint.as_deref(), Some("…E123"));

    assert!(!row.secret_ciphertext.contains("s3cr3t-value"));
    assert!(!row.secret_ciphertext.contains("AKIAEXAMPLE123"));
    let pt = secrets::decrypt(&KEY, &row.secret_ciphertext, &row.secret_nonce).unwrap();
    let v: serde_json::Value = serde_json::from_slice(&pt).unwrap();
    assert_eq!(v["access_key_id"], "AKIAEXAMPLE123");
    assert_eq!(v["secret_access_key"], "s3cr3t-value");

    let (ct2, nonce2) = encrypt_keys("AKIAEXAMPLE123", "rotated");
    let updated = AppStorageUpsert {
        bucket_name: "bucket-2".to_string(),
        secret_ciphertext: ct2,
        secret_nonce: nonce2,
        ..input.clone()
    };
    db.upsert_app_storage(&updated).await.unwrap();
    let row2 = db.get_app_storage(DEFAULT_APP_ID).await.unwrap().unwrap();
    assert_eq!(row2.bucket_name, "bucket-2");

    assert!(db.delete_app_storage(DEFAULT_APP_ID).await.unwrap());
    assert!(db.get_app_storage(DEFAULT_APP_ID).await.unwrap().is_none());
    assert!(!db.delete_app_storage(DEFAULT_APP_ID).await.unwrap());
}
