//! Signed links and the file route over a real S3-compatible bucket.
//!
//! Gated on `LITEGEN_S3_TEST_ENDPOINT`, as the Postgres suite is on
//! `LITEGEN_PG_TEST_URL`: without it every test here no-ops. Run against a
//! throwaway S3-compatible server. CI uses Zenko CloudServer (MinIO no longer
//! publishes images), which checks SigV4 and presigned URLs:
//!   docker run -d --rm -p 127.0.0.1:9555:8000 -e REMOTE_MANAGEMENT_DISABLE=1 zenko/cloudserver
//!   LITEGEN_S3_TEST_ENDPOINT=http://127.0.0.1:9555 LITEGEN_S3_TEST_KEY=accessKey1 \
//!     LITEGEN_S3_TEST_SECRET=verySecretKey1 cargo test --test output_access_s3
//! (`LITEGEN_S3_TEST_KEY` / `LITEGEN_S3_TEST_SECRET` default to MinIO's
//! `minioadmin`.) Each test makes its own private bucket and removes it.

mod harness;

use axum::body::Body;
use axum::http::{Request, StatusCode};
use litegen::api::middleware::DEFAULT_APP_ID;
use litegen::db::DatabaseStore;
use tower::ServiceExt;

struct TestBucket {
    bucket: Box<s3::Bucket>,
    endpoint: String,
    key: String,
    secret: String,
}

impl TestBucket {
    /// A new, private bucket (MinIO's default: no anonymous access).
    async fn create() -> Option<TestBucket> {
        let Ok(endpoint) = std::env::var("LITEGEN_S3_TEST_ENDPOINT") else {
            eprintln!("skipping: set LITEGEN_S3_TEST_ENDPOINT to a throwaway S3-compatible server (see the module docs)");
            return None;
        };
        let key = std::env::var("LITEGEN_S3_TEST_KEY").unwrap_or_else(|_| "minioadmin".into());
        let secret = std::env::var("LITEGEN_S3_TEST_SECRET").unwrap_or_else(|_| "minioadmin".into());
        let name = format!("litegen-access-{}", uuid::Uuid::new_v4().simple());
        let region = s3::Region::Custom { region: "us-east-1".into(), endpoint: endpoint.clone() };
        let creds = s3::creds::Credentials::new(Some(&key), Some(&secret), None, None, None).unwrap();
        let created = s3::Bucket::create_with_path_style(&name, region, creds, s3::BucketConfiguration::default())
            .await
            .expect("create the test bucket");
        assert!(created.success(), "create bucket: {}", created.response_text);
        Some(TestBucket { bucket: created.bucket, endpoint, key, secret })
    }

    fn storage_config(&self) -> litegen::config::S3StorageConfig {
        litegen::config::S3StorageConfig {
            bucket_name: self.bucket.name(),
            region: "us-east-1".into(),
            access_key_id: Some(self.key.clone()),
            secret_access_key: Some(self.secret.clone()),
            endpoint_url: Some(self.endpoint.clone()),
            custom_public_url: None,
            session_token: None,
            credentials_expire_at: None,
        }
    }

    async fn remove(self) {
        if let Ok(objects) = objects_under(&self.bucket, "").await {
            for object in objects {
                let _ = self.bucket.delete_object(&object.key).await;
            }
        }
        let _ = self.bucket.delete().await;
    }
}

/// Every object under `prefix`, page by page until a page says it is the
/// last. Not `Bucket::list`: it pages until no continuation token comes
/// back, and CloudServer's last page carries an empty one (with
/// `IsTruncated` false), which `list` follows forever.
async fn objects_under(bucket: &s3::Bucket, prefix: &str) -> Result<Vec<s3::serde_types::Object>, s3::error::S3Error> {
    let mut objects = Vec::new();
    let mut token = None;
    loop {
        let (page, _) = bucket.list_page(prefix.to_string(), None, token, None, None).await?;
        objects.extend(page.contents);
        match page.next_continuation_token {
            Some(next) if page.is_truncated && !next.is_empty() => token = Some(next),
            _ => return Ok(objects),
        }
    }
}

async fn finished_3d(app: &axum::Router) -> serde_json::Value {
    let id = harness::submit(app, "mock/mesh-3d", "a chair").await;
    let done = harness::poll_until_terminal(app, &id).await;
    assert_eq!(done["status"], "completed", "{done}");
    done
}

#[tokio::test]
async fn a_private_bucket_opens_only_through_litegens_links() {
    let Some(bucket) = TestBucket::create().await else { return };
    let dir = tempfile::tempdir().unwrap();
    let s3 = bucket.storage_config();
    let (app, db) = harness::app_with(dir.path(), |c| {
        c.image_storage.backend = "s3".into();
        c.image_storage.s3 = Some(s3);
        c.storage_access.mode = litegen::access::AccessMode::Signed;
        c.storage_access.signed_url_ttl_seconds = 120;
    })
    .await;

    // The mesh lands in the bucket, and the answer carries a presigned link.
    let done = finished_3d(&app).await;
    let link = done["assets"][0]["url"].as_str().unwrap().to_string();
    assert!(link.starts_with(&bucket.endpoint) && link.contains("X-Amz-Signature="), "{link}");
    assert!(link.contains("X-Amz-Expires=120"), "{link}");
    assert!(done["assets"][0]["expires_at"].is_string(), "{done}");

    // The bucket is private: the stored URL alone is refused by S3 itself...
    let stored = link.split('?').next().unwrap();
    assert_eq!(reqwest::get(stored).await.unwrap().status(), 403, "{stored}");
    // ...and litegen's link opens it, with no credentials of litegen's.
    let fetched = reqwest::get(&link).await.unwrap();
    assert_eq!(fetched.status(), 200, "{link}");
    assert_eq!(&fetched.bytes().await.unwrap()[..4], b"glTF");

    // Private: litegen's file route streams it from the bucket.
    db.put_app_file_access(&litegen::types::AppFileAccessUpsert {
        app_id: DEFAULT_APP_ID.into(),
        mode: Some("private".into()),
        owner_scope: None,
        signed_url_ttl_seconds: None,
        keep_outputs: None,
        retention_days: None,
        signed_links: None,
    })
    .await
    .unwrap();
    let id = done["id"].as_str().unwrap();
    let resp = app.clone().oneshot(Request::get(format!("/v1/generations/{id}/assets/0")).body(Body::empty()).unwrap()).await.unwrap();
    assert_eq!(resp.status(), StatusCode::OK);
    let bytes = axum::body::to_bytes(resp.into_body(), usize::MAX).await.unwrap();
    assert_eq!(&bytes[..4], b"glTF");
    let row = harness::json_body(
        app.clone().oneshot(Request::get(format!("/v1/generations/{id}")).body(Body::empty()).unwrap()).await.unwrap(),
    )
    .await;
    assert_eq!(row["result_url"], format!("http://127.0.0.1:4000/v1/generations/{id}/assets/0"), "{row}");

    bucket.remove().await;
}

#[tokio::test]
async fn deleting_a_generation_deletes_its_objects_and_litegen_links_revoke() {
    let Some(bucket) = TestBucket::create().await else { return };
    let dir = tempfile::tempdir().unwrap();
    let s3 = bucket.storage_config();
    let (app, _db) = harness::app_with(dir.path(), |c| {
        c.image_storage.backend = "s3".into();
        c.image_storage.s3 = Some(s3);
        c.storage_access.mode = litegen::access::AccessMode::Signed;
        c.storage_access.signed_links = litegen::access::SignedLinks::Litegen;
        c.secrets_key = Some(base64::Engine::encode(&base64::engine::general_purpose::STANDARD, [9u8; 32]));
    })
    .await;
    let done = finished_3d(&app).await;
    let id = done["id"].as_str().unwrap().to_string();
    let send = |method: &str, uri: String| {
        let app = app.clone();
        let req = Request::builder().method(method).uri(uri).body(Body::empty()).unwrap();
        async move { app.oneshot(req).await.unwrap() }
    };

    // litegen's own link to an object in a real bucket: served, then revoked.
    let link = done["assets"][0]["url"].as_str().unwrap().to_string();
    let path = &link[link.find("/v1/").unwrap()..];
    assert_eq!(send("GET", path.to_string()).await.status(), StatusCode::OK, "{link}");
    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
    assert!(send("POST", format!("/v1/generations/{id}/revoke-links")).await.status().is_success());
    assert_eq!(send("GET", path.to_string()).await.status(), StatusCode::FORBIDDEN, "revoked");

    // Deleting the generation deletes its objects from the bucket.
    let objects = |b: &s3::Bucket| {
        let b = b.clone();
        let prefix = format!("litegen/3d/{id}/");
        async move { objects_under(&b, &prefix).await.unwrap().len() }
    };
    assert!(objects(&bucket.bucket).await > 0, "the mesh is in the bucket");
    assert_eq!(send("DELETE", format!("/v1/generations/{id}")).await.status(), StatusCode::OK);
    assert_eq!(objects(&bucket.bucket).await, 0, "and gone from it");
    bucket.remove().await;
}

/// Review 2026-11-11 P4 and P6: a kept video streams from the vendor to a
/// spool file and into the bucket in one presigned PUT with its length, and
/// lands whole; litegen then reads it back as a stream, and one range of it.
#[tokio::test]
async fn a_kept_video_lands_in_the_bucket_whole() {
    use futures::StreamExt;
    use litegen::proxy::storage::{ByteRange, ImageStorage, S3Storage, Served};
    use sha2::Digest;
    let Some(bucket) = TestBucket::create().await else { return };
    let video: Vec<u8> = (0..3 * 1024 * 1024u32).map(|i| (i % 251) as u8).collect();
    let vendor = wiremock::MockServer::start().await;
    wiremock::Mock::given(wiremock::matchers::method("GET"))
        .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(video.clone(), "video/mp4"))
        .mount(&vendor)
        .await;
    let config = litegen::config::ImageStorageConfig { backend: "s3".into(), s3: Some(bucket.storage_config()), ..Default::default() };
    for (name, store) in [("operator", S3Storage::from_config(&config).unwrap()), ("guarded", S3Storage::from_config(&config).unwrap().guarded())] {
        let store: std::sync::Arc<dyn ImageStorage> = std::sync::Arc::new(store);
        let id = format!("g-{name}");
        let url = format!("{}/v.mp4", vendor.uri());
        let Ok(kept) = litegen::proxy::router::rehost_video(&store, None, &id, None, Some(&url), None, true, false).await else {
            panic!("{name}: the video was not kept")
        };
        assert_eq!(kept.asset.size_bytes, Some(video.len() as u64), "{name}");
        assert_eq!(kept.asset.sha256.as_deref(), Some(hex::encode(sha2::Sha256::digest(&video)).as_str()), "{name}");
        let key = format!("litegen/video/{id}/video.mp4");
        let stored = bucket.bucket.get_object(&key).await.expect("read back");
        assert_eq!(stored.status_code(), 200, "{name}");
        assert!(stored.bytes().as_ref() == video.as_slice(), "{name}: the bucket holds the whole video");

        let file = store.open(&key, None).await.unwrap().expect("found");
        assert_eq!((file.served, file.len, file.accepts_ranges), (Served::Whole, Some(video.len() as u64), true), "{name}");
        let mut body = file.body;
        let mut read = Vec::new();
        while let Some(chunk) = body.next().await {
            read.extend_from_slice(&chunk.unwrap());
        }
        assert!(read == video, "{name}: streamed back whole");
        let part = store.open(&key, Some(ByteRange::FromTo(10, 13))).await.unwrap().expect("found");
        assert_eq!((part.served, part.len), (Served::Part { start: 10, end: 13 }, Some(video.len() as u64)), "{name}");
        let mut body = part.body;
        assert_eq!(body.next().await.unwrap().unwrap().as_ref(), &video[10..14], "{name}");
        let past = store.open(&key, Some(ByteRange::From(video.len() as u64))).await.unwrap().expect("found");
        assert_eq!(past.served, Served::Unsatisfiable, "{name}");
    }
    bucket.remove().await;
}
