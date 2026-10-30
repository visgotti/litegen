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

/// Where a conversion would park a reference image a vendor must fetch by URL,
/// and the `Cleanup` guard deletes afterwards.
///
/// No conversion uses it today: an uploaded image bound for a URL-taking vendor
/// travels inline as a `data:` URI instead. Parking it here only works for a
/// store that mints publicly fetchable URLs AND for a vendor that fetches before
/// the submit returns — and async vendors fetch later, after `Cleanup` has run.
/// A future presigned-URL path must keep the object alive until the job ends.
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

#[derive(Default)]
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

impl Default for Cleanup {
    fn default() -> Self { Self::empty() }
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
    /// Publishes an uploaded reference for a vendor that refuses `data:` URIs
    /// (`ref_inputs.data_uri: false`). Set once the asset store exists.
    publisher: std::sync::OnceLock<Arc<crate::proxy::ref_publish::RefPublisher>>,
}

impl Materializer {
    pub fn new(storage: Arc<dyn TempStorage>, http: reqwest::Client) -> Self {
        Self { storage, http, publisher: std::sync::OnceLock::new() }
    }

    /// The same materializer, publishing references with `publisher`.
    pub fn with_publisher(self, publisher: Arc<crate::proxy::ref_publish::RefPublisher>) -> Self {
        let _ = self.publisher.set(publisher);
        self
    }

    /// Publish references with `publisher` from now on (once; later calls
    /// are ignored).
    pub fn set_publisher(&self, publisher: Arc<crate::proxy::ref_publish::RefPublisher>) {
        let _ = self.publisher.set(publisher);
    }

    /// The publisher, for the route that serves what it published.
    pub fn publisher(&self) -> Option<&Arc<crate::proxy::ref_publish::RefPublisher>> {
        self.publisher.get()
    }

    /// An uploaded reference as the URL a URL-taking vendor receives: inline
    /// as a `data:` URI where it takes one, else published at litegen's own
    /// URL (L4).
    async fn as_url(&self, bytes: &[u8], mime: &str, data_uri: bool) -> Result<String, MaterializeError> {
        if data_uri {
            return Ok(format!("data:{mime};base64,{}", B64.encode(bytes)));
        }
        let publisher = self.publisher.get().ok_or_else(|| {
            MaterializeError::Ref("this model fetches its references by URL and litegen cannot publish one here; send a public image URL".into())
        })?;
        publisher.publish(bytes, mime).await.map_err(MaterializeError::Ref)
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
        for r in refs {
            let role = r.role.clone().or_else(|| ri.default_role.clone())
                .ok_or_else(|| MaterializeError::Ref("ref missing role".into()))?;
            let form = self.convert(&role, r, &ri.provider_format, ri.data_uri, ctx).await?;
            out_refs.push(MaterializedRef { role, form });
        }
        // No conversion uploads anything any more (see the `→ Url` arms in
        // `convert`), so there is nothing to clean up. The guard stays wired to
        // the store so a future conversion that does upload is cleaned up
        // without touching every caller.
        Ok(MaterializedRequest {
            refs: out_refs,
            cleanup: Cleanup { storage: Some(self.storage.clone()), keys: Vec::new() },
        })
    }

    async fn convert(
        &self,
        role: &str,
        r: ReferenceImage,
        target: &RefProviderFormat,
        data_uri: bool,
        ctx: &MaterializeContext,
    ) -> Result<MaterializedRefForm, MaterializeError> {
        // A generation reference is read back into bytes by the handler, which
        // holds the storage and the caller's access, before this runs.
        match (r.kind, target) {
            (RefImageKind::Generation, _) => Err(MaterializeError::Ref(format!(
                "reference '{}' names a generation that was not read before materialising", r.value
            ))),
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
            // An uploaded image bound for a URL-taking vendor travels INLINE, as a
            // `data:` URI. Uploading it to temp storage instead cannot work: the
            // local store mints `local://` URLs no vendor can fetch, and even a
            // public bucket would race an async vendor — the temp object is
            // deleted when the submit returns, while a queued job (fal) fetches
            // its input only once a runner picks it up.
            // @see <https://fal.ai/docs/documentation/model-apis/fal-cdn> (data URIs)
            (RefImageKind::Base64, RefProviderFormat::Url) => {
                let raw = strip_data_prefix(&r.value);
                let bytes = B64.decode(raw).map_err(|e| MaterializeError::Decode(e.to_string()))?;
                if data_uri {
                    // The URI carries the caller's text, already in the form a
                    // data URI needs.
                    return Ok(MaterializedRefForm::Url(base64_data_uri(raw, data_uri_mime(&r.value))));
                }
                let mime = sniff_image_mime(&bytes).or_else(|| data_uri_mime(&r.value)).unwrap_or("image/png");
                Ok(MaterializedRefForm::Url(self.as_url(&bytes, mime, false).await?))
            }
            (RefImageKind::Base64, RefProviderFormat::Multipart(crate::capabilities::RefProviderFormatMultipart { field_map })) => {
                let raw = strip_data_prefix(&r.value);
                let bytes = B64.decode(raw).map_err(|e| MaterializeError::Decode(e.to_string()))?;
                let fname = field_map.get(role).cloned()
                    .ok_or_else(|| MaterializeError::Ref(format!("no field_map entry for role '{}'", role)))?;
                Ok(MaterializedRefForm::MultipartField { field_name: fname, bytes: bytes.into(), content_type: "application/octet-stream".into() })
            }
            // Inline for the same reason as a base64 upload above.
            (RefImageKind::Blob, RefProviderFormat::Url) => {
                let (bytes, ct) = ctx.blob_parts.get(&r.value).cloned().ok_or_else(|| {
                    MaterializeError::Ref(format!("blob part '{}' not present", r.value))
                })?;
                let mime = sniff_image_mime(&bytes).or_else(|| image_mime(&ct)).unwrap_or("application/octet-stream");
                Ok(MaterializedRefForm::Url(self.as_url(&bytes, mime, data_uri).await?))
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
        // SSRF guard: the URL comes straight from the request body
        // (reference_images[].value). Reject non-public targets before issuing
        // the request. The production client also disables redirect following
        // (see main.rs) so a public host can't 3xx-redirect into a private one.
        crate::util::ssrf::validate_public_url(url)
            .await
            .map_err(MaterializeError::Fetch)?;
        let resp = self.http.get(url).send().await.map_err(|e| MaterializeError::Fetch(e.to_string()))?;
        if !resp.status().is_success() {
            return Err(MaterializeError::Fetch(format!("status {}", resp.status())));
        }
        read_body_capped(resp, MAX_REF_IMAGE_BYTES).await
    }
}

/// Maximum size of a server-side reference-image fetch. The URL is caller-
/// supplied, so an unbounded `resp.bytes()` would let a hostile or misbehaving
/// host stream an arbitrarily large body straight into memory.
const MAX_REF_IMAGE_BYTES: usize = 20 * 1024 * 1024; // 20 MiB

/// Read a response body into memory, aborting once it exceeds `max_bytes`.
///
/// The declared `Content-Length` is rejected up front (cheap), and the cap is
/// re-enforced while streaming so a missing or dishonest length can't bypass it.
async fn read_body_capped(
    resp: reqwest::Response,
    max_bytes: usize,
) -> Result<Vec<u8>, MaterializeError> {
    if let Some(len) = resp.content_length() {
        if len > max_bytes as u64 {
            return Err(MaterializeError::Fetch(format!(
                "reference image too large: {len} bytes exceeds {max_bytes} limit"
            )));
        }
    }
    let mut resp = resp;
    let mut buf: Vec<u8> = Vec::new();
    while let Some(chunk) = resp
        .chunk()
        .await
        .map_err(|e| MaterializeError::Fetch(e.to_string()))?
    {
        if buf.len() + chunk.len() > max_bytes {
            return Err(MaterializeError::Fetch(format!(
                "reference image exceeds {max_bytes} byte limit"
            )));
        }
        buf.extend_from_slice(&chunk);
    }
    Ok(buf)
}

pub fn strip_data_prefix(s: &str) -> &str {
    if let Some(rest) = s.strip_prefix("data:") {
        if let Some(idx) = rest.find(',') {
            return &rest[idx + 1..];
        }
    }
    s
}

/// The MIME type of an image, read from its leading bytes. `None` for anything
/// that is not PNG, JPEG, GIF or WebP.
pub fn sniff_image_mime(bytes: &[u8]) -> Option<&'static str> {
    match bytes {
        [0x89, b'P', b'N', b'G', ..] => Some("image/png"),
        [0xFF, 0xD8, 0xFF, ..] => Some("image/jpeg"),
        [b'G', b'I', b'F', b'8', ..] => Some("image/gif"),
        [b'R', b'I', b'F', b'F', _, _, _, _, b'W', b'E', b'B', b'P', ..] => Some("image/webp"),
        _ => None,
    }
}

/// `ct` when it is one of the image types `sniff_image_mime` knows — a caller's
/// claim is only trusted as far as a vendor will.
fn image_mime(ct: &str) -> Option<&'static str> {
    match ct.split(';').next().unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "image/png" => Some("image/png"),
        "image/jpeg" | "image/jpg" => Some("image/jpeg"),
        "image/gif" => Some("image/gif"),
        "image/webp" => Some("image/webp"),
        _ => None,
    }
}

/// The MIME type a `data:` URI declares, when `s` is one.
fn data_uri_mime(s: &str) -> Option<&str> {
    let meta = s.strip_prefix("data:")?.split(',').next()?;
    meta.split(';').next().filter(|m| !m.is_empty())
}

/// An inline `data:` URI for base64 image content (standard alphabet, no
/// prefix) — the form a URL-taking vendor accepts for an uploaded image.
///
/// The MIME type comes from the bytes themselves; a caller-declared `hint` is
/// used only when the bytes are not a recognised image, and only if it names
/// one. Only the first 16 characters are decoded, which is enough for every
/// signature `sniff_image_mime` checks.
pub fn base64_data_uri(b64: &str, hint: Option<&str>) -> String {
    let head: String = b64.chars().take(16).collect();
    let sniffed = B64.decode(&head).ok().and_then(|b| sniff_image_mime(&b));
    let mime = sniffed
        .or_else(|| hint.and_then(image_mime))
        .unwrap_or("application/octet-stream");
    format!("data:{mime};base64,{b64}")
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

#[cfg(test)]
mod fetch_cap_tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    #[tokio::test]
    async fn rejects_body_larger_than_cap() {
        let server = MockServer::start().await;
        let big = vec![b'x'; 5000];
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(big))
            .mount(&server)
            .await;

        let resp = reqwest::Client::new().get(server.uri()).send().await.unwrap();
        // A 5000-byte body must be rejected under a 1000-byte cap rather than
        // being read fully into memory.
        assert!(
            read_body_capped(resp, 1000).await.is_err(),
            "an over-cap reference body must be rejected, not buffered"
        );
    }

    #[tokio::test]
    async fn accepts_body_within_cap() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_bytes(vec![b'y'; 500]))
            .mount(&server)
            .await;

        let resp = reqwest::Client::new().get(server.uri()).send().await.unwrap();
        let body = read_body_capped(resp, 1000).await.expect("within-cap body should read");
        assert_eq!(body.len(), 500);
    }
}
