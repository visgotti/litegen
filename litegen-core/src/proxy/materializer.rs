use std::sync::Arc;
use std::collections::HashMap;
use bytes::Bytes;
use base64::{Engine, engine::general_purpose::STANDARD as B64};

use crate::capabilities::*;
use crate::types::*;

#[derive(Debug, thiserror::Error)]
pub enum MaterializeError {
    #[error("ref image: {0}")]
    Ref(String),

    #[error("upload failed: {0}")]
    Upload(String),

    #[error("fetch failed: {0}")]
    Fetch(String),

    #[error("decode failed: {0}")]
    Decode(String),
}

#[async_trait::async_trait]
pub trait TempStorage: Send + Sync {
    async fn put(&self, key: &str, bytes: Bytes, content_type: &str) -> Result<String, MaterializeError>;
    async fn delete(&self, key: &str) -> Result<(), MaterializeError>;
}

#[derive(Debug, Default, Clone)]
pub struct MaterializeContext {
    /// Blob field name → (bytes, content type). Populated by the multipart extractor.
    pub blob_parts: HashMap<String, (Bytes, String)>,
}

#[derive(Debug)]
pub struct MaterializedRef {
    pub role: String,
    pub form: MaterializedRefForm,
}

#[derive(Debug)]
pub enum MaterializedRefForm {
    Url(String),
    Base64(String),
    MultipartField { field_name: String, bytes: Bytes, content_type: String },
}

pub struct MaterializedRequest {
    pub refs: Vec<MaterializedRef>,
    /// Drop guard: cleans up temp-uploaded objects when this is dropped.
    pub cleanup: Cleanup,
}

pub struct Cleanup {
    storage: Option<Arc<dyn TempStorage>>,
    keys: Vec<String>,
}

impl Cleanup {
    pub fn empty() -> Self { Self { storage: None, keys: Vec::new() } }
}

impl Drop for Cleanup {
    fn drop(&mut self) {
        if self.keys.is_empty() { return; }
        let Some(storage) = self.storage.clone() else { return; };
        let keys = std::mem::take(&mut self.keys);
        tokio::spawn(async move {
            for k in keys {
                if let Err(e) = storage.delete(&k).await {
                    tracing::warn!(error=%e, key=%k, "temp ref cleanup failed");
                }
            }
        });
    }
}

pub struct Materializer {
    storage: Arc<dyn TempStorage>,
    http: reqwest::Client,
}

impl Materializer {
    pub fn new(storage: Arc<dyn TempStorage>, http: reqwest::Client) -> Self {
        Self { storage, http }
    }

    #[tracing::instrument(
        skip(self, schema, refs, ctx),
        fields(model = %schema.id, ref_count = refs.len())
    )]
    pub async fn materialize(
        &self,
        schema: &ModelSchema,
        refs: Vec<ReferenceImage>,
        ctx: &MaterializeContext,
    ) -> Result<MaterializedRequest, MaterializeError> {
        let Some(ri) = schema.ref_inputs.as_ref() else {
            return Ok(MaterializedRequest {
                refs: Vec::new(),
                cleanup: Cleanup { storage: None, keys: Vec::new() },
            });
        };
        let mut out_refs = Vec::with_capacity(refs.len());
        let mut keys = Vec::new();
        for r in refs {
            let role = r.role.clone().or_else(|| ri.default_role.clone())
                .ok_or_else(|| MaterializeError::Ref("ref missing role".into()))?;
            let form = self.convert(&role, r, &ri.provider_format, ctx, &mut keys).await?;
            out_refs.push(MaterializedRef { role, form });
        }
        Ok(MaterializedRequest {
            refs: out_refs,
            cleanup: Cleanup { storage: Some(self.storage.clone()), keys },
        })
    }

    async fn convert(
        &self,
        role: &str,
        r: ReferenceImage,
        target: &RefProviderFormat,
        ctx: &MaterializeContext,
        keys: &mut Vec<String>,
    ) -> Result<MaterializedRefForm, MaterializeError> {
        match (r.kind, target) {
            (RefImageKind::Url, RefProviderFormat::Url) => Ok(MaterializedRefForm::Url(r.value)),
            (RefImageKind::Base64, RefProviderFormat::Base64) => {
                Ok(MaterializedRefForm::Base64(strip_data_prefix(&r.value).to_string()))
            }
            (RefImageKind::Blob, RefProviderFormat::Multipart(crate::capabilities::RefProviderFormatMultipart { field_map })) => {
                let (bytes, ct) = ctx.blob_parts.get(&r.value).cloned().ok_or_else(|| {
                    MaterializeError::Ref(format!("blob part '{}' not present", r.value))
                })?;
                let fname = field_map.get(role).cloned()
                    .ok_or_else(|| MaterializeError::Ref(format!("no field_map entry for role '{}'", role)))?;
                Ok(MaterializedRefForm::MultipartField { field_name: fname, bytes, content_type: ct })
            }
            (RefImageKind::Url, RefProviderFormat::Base64) => {
                let bytes = self.fetch_url(&r.value).await?;
                Ok(MaterializedRefForm::Base64(B64.encode(&bytes)))
            }
            (RefImageKind::Url, RefProviderFormat::Multipart(crate::capabilities::RefProviderFormatMultipart { field_map })) => {
                let bytes = self.fetch_url(&r.value).await?;
                let fname = field_map.get(role).cloned()
                    .ok_or_else(|| MaterializeError::Ref(format!("no field_map entry for role '{}'", role)))?;
                Ok(MaterializedRefForm::MultipartField { field_name: fname, bytes: bytes.into(), content_type: "application/octet-stream".into() })
            }
            (RefImageKind::Base64, RefProviderFormat::Url) => {
                let raw = strip_data_prefix(&r.value);
                let bytes = B64.decode(raw).map_err(|e| MaterializeError::Decode(e.to_string()))?;
                let key = format!("tmp/refs/{}", uuid::Uuid::new_v4());
                let url = self.storage.put(&key, bytes.into(), "application/octet-stream").await?;
                keys.push(key);
                Ok(MaterializedRefForm::Url(url))
            }
            (RefImageKind::Base64, RefProviderFormat::Multipart(crate::capabilities::RefProviderFormatMultipart { field_map })) => {
                let raw = strip_data_prefix(&r.value);
                let bytes = B64.decode(raw).map_err(|e| MaterializeError::Decode(e.to_string()))?;
                let fname = field_map.get(role).cloned()
                    .ok_or_else(|| MaterializeError::Ref(format!("no field_map entry for role '{}'", role)))?;
                Ok(MaterializedRefForm::MultipartField { field_name: fname, bytes: bytes.into(), content_type: "application/octet-stream".into() })
            }
            (RefImageKind::Blob, RefProviderFormat::Url) => {
                let (bytes, ct) = ctx.blob_parts.get(&r.value).cloned().ok_or_else(|| {
                    MaterializeError::Ref(format!("blob part '{}' not present", r.value))
                })?;
                let key = format!("tmp/refs/{}", uuid::Uuid::new_v4());
                let url = self.storage.put(&key, bytes, &ct).await?;
                keys.push(key);
                Ok(MaterializedRefForm::Url(url))
            }
            (RefImageKind::Blob, RefProviderFormat::Base64) => {
                let (bytes, _ct) = ctx.blob_parts.get(&r.value).cloned().ok_or_else(|| {
                    MaterializeError::Ref(format!("blob part '{}' not present", r.value))
                })?;
                Ok(MaterializedRefForm::Base64(B64.encode(&bytes)))
            }
        }
    }

    async fn fetch_url(&self, url: &str) -> Result<Vec<u8>, MaterializeError> {
        let resp = self.http.get(url).send().await.map_err(|e| MaterializeError::Fetch(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(MaterializeError::Fetch(format!("status {}", resp.status())));
        }
        let bytes = resp.bytes().await.map_err(|e| MaterializeError::Fetch(e.to_string()))?;
        Ok(bytes.to_vec())
    }
}

pub fn strip_data_prefix(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix("data:") {
        if let Some(idx) = rest.find(',') {
            return &rest[idx + 1..];
        }
    }
    s
}

/// Adapter that wraps the existing storage backend with the TempStorage trait.
pub struct StorageAdapter {
    inner: Arc<dyn crate::proxy::storage::ImageStorage>,
}

impl StorageAdapter {
    pub fn new(inner: Arc<dyn crate::proxy::storage::ImageStorage>) -> Self {
        Self { inner }
    }
}

#[async_trait::async_trait]
impl TempStorage for StorageAdapter {
    async fn put(&self, key: &str, bytes: Bytes, ct: &str) -> Result<String, MaterializeError> {
        self.inner.put(key, &bytes, ct).await
            .map_err(|e| MaterializeError::Upload(e.to_string()))
    }
    async fn delete(&self, key: &str) -> Result<(), MaterializeError> {
        self.inner.delete(key).await
            .map_err(|e| MaterializeError::Upload(e.to_string()))
    }
}
