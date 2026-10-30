use std::sync::Arc;
use tracing::{error, info};

use crate::config::ImageStorageConfig;

/// Trait for storing generated images and returning accessible URLs.
#[async_trait::async_trait]
pub trait ImageStore: Send + Sync {
    /// Store image bytes and return a URL where the image can be accessed.
    /// Returns `None` if storage is not available (falls back to b64).
    async fn store(
        &self,
        data: &[u8],
        content_type: &str,
        generation_id: &str,
    ) -> Result<String, ImageStoreError>;
}

#[derive(Debug, thiserror::Error)]
pub enum ImageStoreError {
    #[error("S3 upload failed: {0}")]
    UploadFailed(String),
    #[error("Storage not configured")]
    NotConfigured,
}

/// In-memory / no-op store — images are returned inline as base64 (default behavior).
pub struct LocalStore;

#[async_trait::async_trait]
impl ImageStore for LocalStore {
    async fn store(
        &self,
        _data: &[u8],
        _content_type: &str,
        _generation_id: &str,
    ) -> Result<String, ImageStoreError> {
        // Local store doesn't persist — the router will fall back to b64_json.
        Err(ImageStoreError::NotConfigured)
    }
}

/// S3-compatible object storage backend.
/// Works with AWS S3, MinIO, Cloudflare R2, DigitalOcean Spaces, etc.
pub struct S3Store {
    bucket: s3::Bucket,
    path_prefix: String,
    custom_public_url: Option<String>,
}

impl S3Store {
    pub fn from_config(config: &ImageStorageConfig) -> Result<Self, ImageStoreError> {
        let s3_cfg = config.s3.as_ref().ok_or(ImageStoreError::NotConfigured)?;

        let region = if let Some(ref endpoint) = s3_cfg.endpoint_url {
            s3::Region::Custom {
                region: s3_cfg.region.clone(),
                endpoint: endpoint.clone(),
            }
        } else {
            s3_cfg
                .region
                .parse::<s3::Region>()
                .map_err(|e| ImageStoreError::UploadFailed(format!("Invalid region: {e}")))?
        };

        let credentials = s3::creds::Credentials::new(
            s3_cfg.access_key_id.as_deref(),
            s3_cfg.secret_access_key.as_deref(),
            None, // security token
            None, // session token
            None, // profile
        )
        .map_err(|e| ImageStoreError::UploadFailed(format!("Invalid credentials: {e}")))?;

        let mut bucket = s3::Bucket::new(&s3_cfg.bucket_name, region, credentials)
            .map_err(|e| ImageStoreError::UploadFailed(format!("Failed to create bucket: {e}")))?;

        if s3_cfg.endpoint_url.is_some() {
            bucket = bucket.with_path_style();
        }

        let path_prefix = config
            .path_prefix
            .clone()
            .unwrap_or_else(|| "litegen/images".to_string());

        Ok(Self {
            bucket: *bucket,
            path_prefix,
            custom_public_url: s3_cfg.custom_public_url.clone(),
        })
    }
}

#[async_trait::async_trait]
impl ImageStore for S3Store {
    async fn store(
        &self,
        data: &[u8],
        content_type: &str,
        generation_id: &str,
    ) -> Result<String, ImageStoreError> {
        let ext = match content_type {
            "image/png" => "png",
            "image/jpeg" | "image/jpg" => "jpg",
            "image/webp" => "webp",
            _ => "png",
        };

        let key = format!("{}/{}.{}", self.path_prefix, generation_id, ext);

        let response = self
            .bucket
            .put_object_with_content_type(&key, data, content_type)
            .await
            .map_err(|e| ImageStoreError::UploadFailed(format!("S3 PUT failed: {e}")))?;

        if response.status_code() >= 300 {
            return Err(ImageStoreError::UploadFailed(format!(
                "S3 returned HTTP {}",
                response.status_code()
            )));
        }

        // Build the public URL
        let url = if let Some(ref base) = self.custom_public_url {
            format!("{}/{}", base.trim_end_matches('/'), key)
        } else {
            let region_str = self.bucket.region().to_string();
            let bucket_name = self.bucket.name();
            if self.bucket.is_path_style() {
                format!(
                    "{}/{}/{}",
                    region_str.trim_end_matches('/'),
                    bucket_name,
                    key
                )
            } else {
                format!("https://{}.s3.{}.amazonaws.com/{}", bucket_name, region_str, key)
            }
        };

        info!(key = %key, "Image uploaded to S3");
        Ok(url)
    }
}

/// Extended storage trait that supports arbitrary key-based put/delete
/// (used by the materializer for temporary reference image uploads).
#[async_trait::async_trait]
pub trait ImageStorage: Send + Sync {
    /// Put bytes at a key and return a public URL.
    async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, ImageStoreError>;
    /// Delete an object by key.
    async fn delete(&self, key: &str) -> Result<(), ImageStoreError>;
    /// The bytes stored at `key` and their content type, or `None` when there
    /// is nothing there. Chaining reads a stored source back through this.
    /// Stores that cannot read answer `None`.
    async fn get(&self, _key: &str) -> Result<Option<(bytes::Bytes, String)>, ImageStoreError> {
        Ok(None)
    }
}

/// LocalStorage: put/delete are no-ops (returns a fake URL for local dev).
pub struct LocalStorage;

#[async_trait::async_trait]
impl ImageStorage for LocalStorage {
    async fn put(&self, key: &str, _bytes: &bytes::Bytes, _content_type: &str) -> Result<String, ImageStoreError> {
        Ok(format!("local://{}", key))
    }
    async fn delete(&self, _key: &str) -> Result<(), ImageStoreError> {
        Ok(())
    }
}

/// S3Storage: delegates to the S3 bucket.
pub struct S3Storage {
    bucket: s3::Bucket,
    custom_public_url: Option<String>,
}

impl S3Storage {
    pub fn from_config(config: &ImageStorageConfig) -> Result<Self, ImageStoreError> {
        let s3_cfg = config.s3.as_ref().ok_or(ImageStoreError::NotConfigured)?;
        let region = if let Some(ref endpoint) = s3_cfg.endpoint_url {
            s3::Region::Custom {
                region: s3_cfg.region.clone(),
                endpoint: endpoint.clone(),
            }
        } else {
            s3_cfg.region.parse::<s3::Region>()
                .map_err(|e| ImageStoreError::UploadFailed(format!("Invalid region: {e}")))?
        };
        let credentials = s3::creds::Credentials::new(
            s3_cfg.access_key_id.as_deref(),
            s3_cfg.secret_access_key.as_deref(),
            None, None, None,
        ).map_err(|e| ImageStoreError::UploadFailed(format!("Invalid credentials: {e}")))?;
        let mut bucket = s3::Bucket::new(&s3_cfg.bucket_name, region, credentials)
            .map_err(|e| ImageStoreError::UploadFailed(format!("Failed to create bucket: {e}")))?;
        if s3_cfg.endpoint_url.is_some() { bucket = bucket.with_path_style(); }
        Ok(Self { bucket: *bucket, custom_public_url: s3_cfg.custom_public_url.clone() })
    }

    /// The absolute URL an object at `key` is served from.
    ///
    /// For an S3-compatible endpoint (path-style) this is the bucket's own URL,
    /// `scheme://endpoint-host/bucket/key`: the region's `Display` is only its
    /// name for a custom endpoint, which used to yield a relative
    /// `region/bucket/key` that no client can fetch.
    fn object_url(&self, key: &str) -> String {
        if let Some(ref base) = self.custom_public_url {
            format!("{}/{}", base.trim_end_matches('/'), key)
        } else if self.bucket.is_path_style() {
            format!("{}/{}", self.bucket.url().trim_end_matches('/'), key)
        } else {
            format!("https://{}.s3.{}.amazonaws.com/{}", self.bucket.name(), self.bucket.region(), key)
        }
    }
}

#[async_trait::async_trait]
impl ImageStorage for S3Storage {
    async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, ImageStoreError> {
        let response = self.bucket
            .put_object_with_content_type(key, bytes.as_ref(), content_type)
            .await
            .map_err(|e| ImageStoreError::UploadFailed(format!("S3 PUT failed: {e}")))?;
        if response.status_code() >= 300 {
            return Err(ImageStoreError::UploadFailed(format!("S3 returned HTTP {}", response.status_code())));
        }
        Ok(self.object_url(key))
    }
    async fn delete(&self, key: &str) -> Result<(), ImageStoreError> {
        self.bucket.delete_object(key).await
            .map_err(|e| ImageStoreError::UploadFailed(format!("S3 DELETE failed: {e}")))?;
        Ok(())
    }
    async fn get(&self, key: &str) -> Result<Option<(bytes::Bytes, String)>, ImageStoreError> {
        let response = self.bucket.get_object(key).await
            .map_err(|e| ImageStoreError::UploadFailed(format!("S3 GET failed: {e}")))?;
        match response.status_code() {
            200 => {
                let ext = key.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
                let ct = response
                    .headers()
                    .get("content-type")
                    .cloned()
                    .unwrap_or_else(|| model3d_content_type(ext, "application/octet-stream").to_string());
                Ok(Some((bytes::Bytes::copy_from_slice(response.bytes()), ct)))
            }
            404 => Ok(None),
            code => Err(ImageStoreError::UploadFailed(format!("S3 GET returned HTTP {code}"))),
        }
    }
}

/// Build the appropriate image store from configuration.
pub fn build_image_store(config: &ImageStorageConfig) -> Arc<dyn ImageStore> {
    match config.backend.as_str() {
        "s3" => match S3Store::from_config(config) {
            Ok(store) => {
                info!("Image storage: S3 bucket configured");
                Arc::new(store)
            }
            Err(e) => {
                error!(error = %e, "Failed to configure S3 storage, falling back to local");
                Arc::new(LocalStore)
            }
        },
        _ => {
            info!("Image storage: local (base64 inline)");
            Arc::new(LocalStore)
        }
    }
}

// ─── 3D asset storage ───────────────────────────────────────────────────────

/// Default key prefix for 3D outputs. Distinct from images' `litegen/images` so
/// a shared bucket keeps the two modalities apart.
pub const MODEL3D_PATH_PREFIX: &str = "litegen/3d";

/// Build the storage key for one file of a 3D generation.
///
/// The extension comes from the CALLER, never from a content-type switch — that
/// is the whole reason this path uses `ImageStorage::put` (explicit key) rather
/// than `ImageStore::store`, whose S3 impl would save a `.glb` as `.png`.
pub fn model3d_asset_key(path_prefix: Option<&str>, generation_id: &str, name: &str, ext: &str) -> String {
    let prefix = path_prefix.unwrap_or(MODEL3D_PATH_PREFIX).trim_matches('/');
    let ext = sanitize_model3d_format(ext);
    format!("{prefix}/{generation_id}/{name}.{ext}")
}

/// Normalise a provider-supplied container name into something safe to put in a
/// storage key and a public URL.
///
/// `Model3dFile.format` is a free-form string filled in by each adapter from
/// whatever the vendor said, and it is interpolated straight into an object key
/// — so a value containing `/` or `..` would rewrite the key's path. Lowercases
/// (vendors are inconsistent about `GLB` vs `glb`, and a case difference would
/// otherwise read as a second distinct container), and keeps only
/// `[a-z0-9]`. An empty or fully-rejected value becomes `bin`, which is wrong
/// but inert, rather than producing a key that ends in a bare dot.
pub fn sanitize_model3d_format(format: &str) -> String {
    let cleaned: String = format
        .trim()
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .take(16)
        .collect();
    if cleaned.is_empty() { "bin".to_string() } else { cleaned }
}

/// The content type to store a 3D asset under.
///
/// Derived from the container rather than taken from the adapter, so two
/// adapters cannot tag the same format differently — the browser viewer and any
/// CDN in front of the bucket both key off this. The provider's own value is
/// the fallback for formats not listed here (textures and previews, mostly).
///
/// `fbx` and `ply` are deliberately absent: neither has an IANA registration,
/// and inventing one would be worse than `application/octet-stream`.
pub fn model3d_content_type<'a>(format: &str, provider_value: &'a str) -> &'a str {
    match format {
        "glb" => "model/gltf-binary",
        "gltf" => "model/gltf+json",
        "obj" => "model/obj",
        "mtl" => "model/mtl",
        "stl" => "model/stl",
        "3mf" => "model/3mf",
        "dae" => "model/vnd.collada+xml",
        "usdz" => "model/vnd.usdz+zip",
        "usda" => "model/vnd.usda",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        _ => provider_value,
    }
}

/// Local (no-S3) asset store, on disk.
///
/// Unlike [`LocalStorage`], which returns an unusable `local://` URL, this
/// writes the bytes under `root` and hands back an ABSOLUTE URL served by
/// `GET /v1/models3d/assets/{key}`. Clients fetch mesh URLs from their own
/// workers, where a relative path is indistinguishable from an outage.
///
/// On disk rather than in memory: chaining operations by generation id reads
/// stored sources back, and a restart between two steps must not lose them.
/// [`sweep_expired`] deletes what is older than the configured retention.
pub struct LocalModel3dStorage {
    root: std::path::PathBuf,
    public_base_url: String,
}

impl LocalModel3dStorage {
    pub fn new(root: impl Into<std::path::PathBuf>, public_base_url: String) -> Self {
        Self {
            root: root.into(),
            public_base_url: public_base_url.trim_end_matches('/').to_string(),
        }
    }

    /// The file a key lives in, or an error for a key that is empty, absolute,
    /// or would leave `root` (`..`, `.`, empty segments, backslashes). Keys are
    /// minted by litegen, but an app's configured path prefix flows into them.
    fn path_for(&self, key: &str) -> Result<std::path::PathBuf, ImageStoreError> {
        let bad = || ImageStoreError::UploadFailed(format!("refusing storage key {key:?}"));
        if key.is_empty() || key.starts_with('/') || key.contains('\\') || key.contains('\0') {
            return Err(bad());
        }
        let mut path = self.root.clone();
        for seg in key.split('/') {
            if seg.is_empty() || seg == "." || seg == ".." {
                return Err(bad());
            }
            path.push(seg);
        }
        Ok(path)
    }
}

#[async_trait::async_trait]
impl ImageStorage for LocalModel3dStorage {
    async fn put(&self, key: &str, bytes: &bytes::Bytes, _content_type: &str) -> Result<String, ImageStoreError> {
        let path = self.path_for(key)?;
        let bytes = bytes.clone();
        tokio::task::spawn_blocking(move || -> std::io::Result<()> {
            if let Some(dir) = path.parent() {
                std::fs::create_dir_all(dir)?;
            }
            // Written aside and renamed into place, so a reader never sees a
            // half-written file and a crash leaves no truncated asset behind.
            let tmp = path.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
            std::fs::write(&tmp, &bytes)?;
            std::fs::rename(&tmp, &path)
        })
        .await
        .map_err(|e| ImageStoreError::UploadFailed(format!("local write task failed: {e}")))?
        .map_err(|e| ImageStoreError::UploadFailed(format!("local write failed: {e}")))?;
        Ok(format!("{}/v1/models3d/assets/{}", self.public_base_url, key))
    }
    async fn delete(&self, key: &str) -> Result<(), ImageStoreError> {
        let path = self.path_for(key)?;
        match tokio::fs::remove_file(&path).await {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(ImageStoreError::UploadFailed(format!("local delete failed: {e}"))),
        }
    }
    async fn get(&self, key: &str) -> Result<Option<(bytes::Bytes, String)>, ImageStoreError> {
        let path = self.path_for(key)?;
        match tokio::fs::read(&path).await {
            Ok(bytes) => {
                let ext = key.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
                let ct = model3d_content_type(ext, "application/octet-stream").to_string();
                Ok(Some((bytes::Bytes::from(bytes), ct)))
            }
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(e) => Err(ImageStoreError::UploadFailed(format!("local read failed: {e}"))),
        }
    }
}

/// Delete every file under `root` last modified more than `retention` before
/// `now`, then every directory that leaves empty (never `root` itself).
/// Returns how many files were deleted. Best-effort: an entry that cannot be
/// read or removed is skipped, and the next sweep tries again.
pub fn sweep_expired(root: &std::path::Path, retention: std::time::Duration, now: std::time::SystemTime) -> usize {
    let cutoff = now.checked_sub(retention).unwrap_or(std::time::UNIX_EPOCH);
    let mut removed = 0;
    // Explicit stacks: a directory walk must not recurse on attacker-shaped depth.
    let mut dirs = vec![root.to_path_buf()];
    let mut visited = Vec::new();
    while let Some(dir) = dirs.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else { continue };
        for entry in entries.flatten() {
            let Ok(ft) = entry.file_type() else { continue };
            let path = entry.path();
            if ft.is_dir() {
                dirs.push(path);
            } else if ft.is_file() {
                let expired = entry.metadata().and_then(|m| m.modified()).is_ok_and(|t| t < cutoff);
                if expired && std::fs::remove_file(&path).is_ok() {
                    removed += 1;
                }
            }
        }
        visited.push(dir);
    }
    // Deepest first, so a parent is only tried once its children are gone.
    visited.sort_by_key(|d| std::cmp::Reverse(d.components().count()));
    for dir in visited {
        if dir != root {
            let _ = std::fs::remove_dir(&dir); // fails, harmlessly, unless empty
        }
    }
    removed
}

/// Run [`sweep_expired`] over `root` every hour, off the async runtime.
pub fn spawn_retention_sweeper(root: std::path::PathBuf, retention_days: u32) -> tokio::task::JoinHandle<()> {
    let retention = std::time::Duration::from_secs(u64::from(retention_days) * 86_400);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let root = root.clone();
            match tokio::task::spawn_blocking(move || sweep_expired(&root, retention, std::time::SystemTime::now())).await {
                Ok(n) if n > 0 => info!(removed = n, "asset retention sweep"),
                Ok(_) => {}
                Err(e) => error!(error = %e, "asset retention sweep panicked"),
            }
        }
    })
}

/// Build the 3D asset store from configuration.
///
/// Mirrors [`build_image_store`] but returns the key-explicit [`ImageStorage`]
/// trait, and falls back to [`LocalModel3dStorage`] (absolute URLs) rather than
/// a no-op store, because a 3D generation with no reachable mesh URL is a
/// failed generation, not a degraded one.
pub fn build_model3d_store(config: &ImageStorageConfig, public_base_url: &str) -> Arc<dyn ImageStorage> {
    match config.backend.as_str() {
        "s3" => match S3Storage::from_config(config) {
            Ok(store) => {
                info!("3D asset storage: S3 bucket configured");
                Arc::new(store)
            }
            Err(e) => {
                error!(error = %e, "Failed to configure S3 3D storage, falling back to local");
                Arc::new(LocalModel3dStorage::new(config.local_dir(), public_base_url.to_string()))
            }
        },
        _ => {
            info!(dir = %config.local_dir().display(), "3D asset storage: local disk (served from this process)");
            Arc::new(LocalModel3dStorage::new(config.local_dir(), public_base_url.to_string()))
        }
    }
}

#[cfg(test)]
mod model3d_storage_tests {
    use super::*;

    fn s3(endpoint: Option<&str>, public: Option<&str>) -> S3Storage {
        S3Storage::from_config(&ImageStorageConfig {
            backend: "s3".into(),
            s3: Some(crate::config::S3StorageConfig {
                bucket_name: "litegen".into(),
                region: "fr-par".into(),
                access_key_id: Some("k".into()),
                secret_access_key: Some("s".into()),
                endpoint_url: endpoint.map(str::to_string),
                custom_public_url: public.map(str::to_string),
            }),
            path_prefix: None,
            ..Default::default()
        })
        .unwrap()
    }

    #[test]
    fn an_s3_compatible_endpoint_mints_absolute_object_urls() {
        let key = "litegen/3d/litegen-3d-abc/model.glb";
        assert_eq!(
            s3(Some("https://s3.fr-par.scw.cloud"), None).object_url(key),
            "https://s3.fr-par.scw.cloud/litegen/litegen/3d/litegen-3d-abc/model.glb",
        );
        assert_eq!(
            s3(Some("http://minio:9000"), None).object_url(key),
            "http://minio:9000/litegen/litegen/3d/litegen-3d-abc/model.glb",
        );
        // A custom public URL still wins.
        assert_eq!(
            s3(Some("http://minio:9000"), Some("https://cdn.example.com/")).object_url(key),
            "https://cdn.example.com/litegen/3d/litegen-3d-abc/model.glb",
        );
    }

    #[test]
    fn asset_key_takes_its_extension_from_the_caller_not_the_content_type() {
        // Regression guard for the S3Store bug this path deliberately avoids:
        // `S3Store::store` maps content_type → extension through a hardcoded
        // image switch and would save a mesh as ".png".
        let key = model3d_asset_key(None, "litegen-3d-abc", "model", "glb");
        assert_eq!(key, "litegen/3d/litegen-3d-abc/model.glb");
        assert!(key.ends_with(".glb"));

        let prefixed = model3d_asset_key(Some("tenant-7/meshes"), "litegen-3d-abc", "preview", "png");
        assert_eq!(prefixed, "tenant-7/meshes/litegen-3d-abc/preview.png");
    }

    #[test]
    fn asset_key_default_prefix_is_distinct_from_images() {
        assert_eq!(MODEL3D_PATH_PREFIX, "litegen/3d");
        assert_ne!(MODEL3D_PATH_PREFIX, "litegen/images");
    }

    #[tokio::test]
    async fn local_store_returns_an_absolute_url_and_serves_the_bytes_back() {
        // aipix fetches asset URLs from a worker: a root-relative path fails with
        // "invalid url: relative URL without a base" and is indistinguishable
        // from a provider outage.
        let dir = tempfile::tempdir().unwrap();
        let store = LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:8080".into());
        let key = model3d_asset_key(None, "litegen-3d-local", "model", "glb");
        let url = store
            .put(&key, &bytes::Bytes::from_static(b"glTF\x02\x00\x00\x00"), "model/gltf-binary")
            .await
            .unwrap();

        assert_eq!(url, "http://127.0.0.1:8080/v1/models3d/assets/litegen/3d/litegen-3d-local/model.glb");
        assert!(url.starts_with("http://") || url.starts_with("https://"), "must be absolute");
        assert!(!url.starts_with("local://"));

        let (bytes, ct) = store.get(&key).await.unwrap().expect("bytes are retrievable");
        assert_eq!(&bytes[0..4], b"glTF");
        assert_eq!(ct, "model/gltf-binary");
    }

    #[tokio::test]
    async fn local_store_trims_a_trailing_slash_on_the_base_url() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalModel3dStorage::new(dir.path(), "https://litegen.example.com/".into());
        let url = store
            .put("litegen/3d/x/model.glb", &bytes::Bytes::from_static(b"x"), "model/gltf-binary")
            .await
            .unwrap();
        assert_eq!(url, "https://litegen.example.com/v1/models3d/assets/litegen/3d/x/model.glb");
    }

    /// 3D doc §3A: chaining by generation id reads stored sources, so a
    /// restart between two steps must not lose them.
    #[tokio::test]
    async fn a_store_over_the_same_dir_serves_what_an_earlier_one_stored() {
        let dir = tempfile::tempdir().unwrap();
        let key = model3d_asset_key(None, "litegen-3d-restart", "model", "glb");
        let first = LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:8080".into());
        first.put(&key, &bytes::Bytes::from_static(b"glTF\x02\x00\x00\x00"), "model/gltf-binary").await.unwrap();
        drop(first);
        // Nothing in memory survives a restart; only the directory does.
        let second = LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:8080".into());
        let (bytes, ct) = second.get(&key).await.unwrap().expect("bytes survive a restart");
        assert_eq!(&bytes[..4], b"glTF");
        assert_eq!(ct, "model/gltf-binary");
        assert!(second.get("litegen/3d/litegen-3d-restart/model.obj").await.unwrap().is_none());
    }

    #[tokio::test]
    async fn keys_that_would_leave_the_root_are_refused() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalModel3dStorage::new(dir.path(), "http://h".into());
        for key in ["../escape.glb", "/etc/passwd", "a/../../b.glb", "a\\b.glb", "", "a//b.glb", "./a.glb", "a/.."] {
            assert!(store.put(key, &bytes::Bytes::from_static(b"x"), "x").await.is_err(), "{key:?} was stored");
            assert!(store.get(key).await.unwrap_or(None).is_none(), "{key:?} was read");
        }
        assert!(!dir.path().parent().unwrap().join("escape.glb").exists());
    }

    #[test]
    fn the_sweeper_deletes_only_files_older_than_retention() {
        let dir = tempfile::tempdir().unwrap();
        let old = dir.path().join("litegen/3d/old/model.glb");
        let new = dir.path().join("litegen/3d/new/model.glb");
        for p in [&old, &new] {
            std::fs::create_dir_all(p.parent().unwrap()).unwrap();
            std::fs::write(p, b"x").unwrap();
        }
        let now = std::time::SystemTime::now();
        std::fs::File::options()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(now - std::time::Duration::from_secs(8 * 86_400))
            .unwrap();
        let removed = sweep_expired(dir.path(), std::time::Duration::from_secs(7 * 86_400), now);
        assert_eq!(removed, 1);
        assert!(!old.exists(), "the expired file goes");
        assert!(!old.parent().unwrap().exists(), "and so does the directory it emptied");
        assert!(new.exists(), "a fresh file stays");
        assert!(dir.path().exists(), "the root itself is never removed");
    }

    #[test]
    fn retention_is_never_below_seven_days() {
        let mut cfg = ImageStorageConfig::default();
        assert_eq!(cfg.retention_days(), 7);
        cfg.retention_days = Some(1);
        assert_eq!(cfg.retention_days(), 7);
        cfg.retention_days = Some(30);
        assert_eq!(cfg.retention_days(), 30);
    }

    #[test]
    fn build_model3d_store_falls_back_to_local_when_s3_is_unconfigured() {
        // backend "s3" with no `s3` block is the branch that matters:
        // `S3Storage::from_config` returns `NotConfigured` and the error arm
        // must still hand back a store minting ABSOLUTE urls. A "local" config
        // takes the `_ =>` arm instead and never executes it, so this test used
        // to pass while that arm returned anything at all.
        let cfg = crate::config::ImageStorageConfig { backend: "s3".into(), path_prefix: None, s3: None, ..Default::default() };
        assert!(
            matches!(S3Storage::from_config(&cfg), Err(ImageStoreError::NotConfigured)),
            "precondition: this config must drive the S3 error arm",
        );

        let store = build_model3d_store(&cfg, "http://localhost:8080");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let url = rt
            .block_on(store.put("litegen/3d/y/model.glb", &bytes::Bytes::from_static(b"y"), "model/gltf-binary"))
            .unwrap();
        assert!(url.starts_with("http://localhost:8080/"), "must be absolute, got {url}");
        assert!(!url.starts_with("local://"), "a local:// url is unfetchable by a client: {url}");
    }

    #[test]
    fn build_model3d_store_is_local_when_the_backend_is_local() {
        let cfg = crate::config::ImageStorageConfig { backend: "local".into(), path_prefix: None, s3: None, ..Default::default() };
        let store = build_model3d_store(&cfg, "http://localhost:8080");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let url = rt
            .block_on(store.put("litegen/3d/z/model.glb", &bytes::Bytes::from_static(b"z"), "model/gltf-binary"))
            .unwrap();
        assert_eq!(url, "http://localhost:8080/v1/models3d/assets/litegen/3d/z/model.glb");
    }
}
