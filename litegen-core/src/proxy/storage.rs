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
        let url = if let Some(ref base) = self.custom_public_url {
            format!("{}/{}", base.trim_end_matches('/'), key)
        } else {
            let region_str = self.bucket.region().to_string();
            let bucket_name = self.bucket.name();
            if self.bucket.is_path_style() {
                format!("{}/{}/{}", region_str.trim_end_matches('/'), bucket_name, key)
            } else {
                format!("https://{}.s3.{}.amazonaws.com/{}", bucket_name, region_str, key)
            }
        };
        Ok(url)
    }
    async fn delete(&self, key: &str) -> Result<(), ImageStoreError> {
        self.bucket.delete_object(key).await
            .map_err(|e| ImageStoreError::UploadFailed(format!("S3 DELETE failed: {e}")))?;
        Ok(())
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
    format!("{prefix}/{generation_id}/{name}.{ext}")
}

/// Process-global byte store backing [`LocalModel3dStorage`].
type LocalAssets = tokio::sync::RwLock<std::collections::HashMap<String, (bytes::Bytes, String)>>;
fn local_assets() -> &'static LocalAssets {
    static ASSETS: std::sync::OnceLock<LocalAssets> = std::sync::OnceLock::new();
    ASSETS.get_or_init(|| tokio::sync::RwLock::new(std::collections::HashMap::new()))
}

/// Fetch bytes previously stored by [`LocalModel3dStorage`]. Used by the
/// asset-serving route.
pub async fn local_asset_bytes(key: &str) -> Option<(bytes::Bytes, String)> {
    local_assets().read().await.get(key).cloned()
}

/// Local (no-S3) 3D asset store.
///
/// Unlike [`LocalStorage`], which returns an unusable `local://` URL, this keeps
/// the bytes in-process and hands back an ABSOLUTE URL served by
/// `GET /v1/models3d/assets/{key}`. Clients fetch mesh URLs from their own
/// workers, where a relative path is indistinguishable from an outage.
pub struct LocalModel3dStorage {
    public_base_url: String,
}

impl LocalModel3dStorage {
    pub fn new(public_base_url: String) -> Self {
        Self { public_base_url: public_base_url.trim_end_matches('/').to_string() }
    }
}

#[async_trait::async_trait]
impl ImageStorage for LocalModel3dStorage {
    async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, ImageStoreError> {
        local_assets().write().await.insert(key.to_string(), (bytes.clone(), content_type.to_string()));
        Ok(format!("{}/v1/models3d/assets/{}", self.public_base_url, key))
    }
    async fn delete(&self, key: &str) -> Result<(), ImageStoreError> {
        local_assets().write().await.remove(key);
        Ok(())
    }
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
                Arc::new(LocalModel3dStorage::new(public_base_url.to_string()))
            }
        },
        _ => {
            info!("3D asset storage: local (served from this process)");
            Arc::new(LocalModel3dStorage::new(public_base_url.to_string()))
        }
    }
}

#[cfg(test)]
mod model3d_storage_tests {
    use super::*;

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
        let store = LocalModel3dStorage::new("http://127.0.0.1:8080".into());
        let key = model3d_asset_key(None, "litegen-3d-local", "model", "glb");
        let url = store
            .put(&key, &bytes::Bytes::from_static(b"glTF\x02\x00\x00\x00"), "model/gltf-binary")
            .await
            .unwrap();

        assert_eq!(url, "http://127.0.0.1:8080/v1/models3d/assets/litegen/3d/litegen-3d-local/model.glb");
        assert!(url.starts_with("http://") || url.starts_with("https://"), "must be absolute");
        assert!(!url.starts_with("local://"));

        let (bytes, ct) = local_asset_bytes(&key).await.expect("bytes are retrievable");
        assert_eq!(&bytes[0..4], b"glTF");
        assert_eq!(ct, "model/gltf-binary");
    }

    #[tokio::test]
    async fn local_store_trims_a_trailing_slash_on_the_base_url() {
        let store = LocalModel3dStorage::new("https://litegen.example.com/".into());
        let url = store
            .put("litegen/3d/x/model.glb", &bytes::Bytes::from_static(b"x"), "model/gltf-binary")
            .await
            .unwrap();
        assert_eq!(url, "https://litegen.example.com/v1/models3d/assets/litegen/3d/x/model.glb");
    }

    #[test]
    fn build_model3d_store_falls_back_to_local_when_s3_is_unconfigured() {
        let cfg = crate::config::ImageStorageConfig { backend: "local".into(), path_prefix: None, s3: None };
        // Must not panic and must not be an S3 store; the smoke test is that a
        // put against it yields an absolute URL rather than an error.
        let store = build_model3d_store(&cfg, "http://localhost:8080");
        let rt = tokio::runtime::Builder::new_current_thread().enable_all().build().unwrap();
        let url = rt
            .block_on(store.put("litegen/3d/y/model.glb", &bytes::Bytes::from_static(b"y"), "model/gltf-binary"))
            .unwrap();
        assert!(url.starts_with("http://localhost:8080/"));
    }
}
