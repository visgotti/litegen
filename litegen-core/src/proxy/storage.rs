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
