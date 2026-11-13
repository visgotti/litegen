//! Where the engine puts files and how it names them: the store traits,
//! their error, the local stores (no config needed), the key layout and the
//! content types. The server's S3 stores and the wiring from config stay in
//! proxy/storage.rs, which re-exports this module at its old paths
//! (docs/superpowers/specs/2026-11-07-engine-boundary-design.md D2).

/// What a store needs to sign a link to a file it serves itself: the query
/// string for `key`. The server's `access::signing::LinkSigner` implements
/// it; the engine never holds the signing secret.
pub trait LinkSigning: Send + Sync {
    fn local_query(&self, key: &str, issued: i64, expires: i64) -> String;
}

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

/// One byte range a reader asked for (an HTTP `Range: bytes=…` with a single
/// range). Ends are inclusive, as in HTTP.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ByteRange {
    /// `bytes=a-b`: from `a` to `b`.
    FromTo(u64, u64),
    /// `bytes=a-`: from `a` to the end.
    From(u64),
    /// `bytes=-n`: the last `n` bytes.
    Last(u64),
}

impl ByteRange {
    /// The first and last byte this range picks out of a file of `len`
    /// bytes, or `None` when it picks none (HTTP 416).
    pub fn resolve(&self, len: u64) -> Option<(u64, u64)> {
        match *self {
            ByteRange::FromTo(a, b) if a <= b && a < len => Some((a, b.min(len - 1))),
            ByteRange::From(a) if a < len => Some((a, len - 1)),
            ByteRange::Last(n) if n > 0 && len > 0 => Some((len.saturating_sub(n), len - 1)),
            _ => None,
        }
    }

    /// The `Range` header that asks a server for this range.
    pub fn header_value(&self) -> String {
        match *self {
            ByteRange::FromTo(a, b) => format!("bytes={a}-{b}"),
            ByteRange::From(a) => format!("bytes={a}-"),
            ByteRange::Last(n) => format!("bytes=-{n}"),
        }
    }
}

/// Which part of a stored file a read returned.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Served {
    /// All of it (no range was asked, or the store ignores ranges).
    Whole,
    /// The bytes from `start` to `end`, both included.
    Part { start: u64, end: u64 },
    /// The range asked for lies outside the file: no bytes.
    Unsatisfiable,
}

/// A stored file opened for reading: its bytes arrive as a stream, so a
/// reader never holds the whole file ([`ImageStorage::open`]).
pub struct StoredFile {
    pub content_type: String,
    /// The whole file's length, when the store knows it.
    pub len: Option<u64>,
    pub served: Served,
    /// Whether this store reads just the range asked for (`Accept-Ranges`).
    pub accepts_ranges: bool,
    pub body: futures::stream::BoxStream<'static, std::io::Result<bytes::Bytes>>,
}

/// How much of a file a stream reads at a time.
pub const STREAM_CHUNK: usize = 128 * 1024;

/// `reader`'s bytes as a stream, [`STREAM_CHUNK`] at a time.
pub fn read_chunks(
    reader: impl tokio::io::AsyncRead + Unpin + Send + 'static,
) -> futures::stream::BoxStream<'static, std::io::Result<bytes::Bytes>> {
    use futures::StreamExt;
    use tokio::io::AsyncReadExt;
    futures::stream::try_unfold(reader, |mut reader| async move {
        let mut buf = bytes::BytesMut::with_capacity(STREAM_CHUNK);
        Ok(match reader.read_buf(&mut buf).await? {
            0 => None,
            _ => Some((buf.freeze(), reader)),
        })
    })
    .boxed()
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
    /// The file stored at `key`, opened as a stream of its bytes, or `None`
    /// when there is nothing there: what serving a file to a caller reads,
    /// so it never holds the whole file. With `range`, a store that can
    /// reads only those bytes. This default reads the whole file through
    /// [`ImageStorage::get`] and ignores the range.
    async fn open(&self, key: &str, _range: Option<ByteRange>) -> Result<Option<StoredFile>, ImageStoreError> {
        use futures::StreamExt;
        Ok(self.get(key).await?.map(|(bytes, content_type)| StoredFile {
            content_type,
            len: Some(bytes.len() as u64),
            served: Served::Whole,
            accepts_ranges: false,
            body: futures::stream::once(async move { Ok(bytes) }).boxed(),
        }))
    }
    /// Put the file at `path` at `key` and return its URL, as [`put`](Self::put)
    /// does with bytes: for a file too large to hold in memory (a kept
    /// video). The file is the caller's to remove afterwards, if still there.
    /// This default reads it whole and puts the bytes.
    async fn put_file(&self, key: &str, path: &std::path::Path, content_type: &str) -> Result<String, ImageStoreError> {
        // The path is the log's: this text can end up as a generation's
        // stored error, which its caller reads (payloads review #3).
        let bytes = tokio::fs::read(path).await.map_err(|e| {
            tracing::warn!(path = %path.display(), error = %e, "could not read a file to upload");
            ImageStoreError::UploadFailed("could not read the file to upload".into())
        })?;
        self.put(key, &bytes::Bytes::from(bytes), content_type).await
    }
    /// Where a file bound for this store may wait on disk before
    /// [`put_file`](Self::put_file), when the store has a place of its own
    /// (the local store: beside its files, so the put is a rename).
    fn spool_dir(&self) -> Option<std::path::PathBuf> {
        None
    }
    /// The key a URL this store minted was put under, or `None` for a URL it
    /// did not mint. Lets a stored asset be read back from the URL its
    /// generation row records.
    fn key_of(&self, _url: &str) -> Option<String> {
        None
    }
    /// A link to `key` that works without credentials until `ttl` passes,
    /// and when it expires; `None` when this store cannot make one.
    async fn signed_url(
        &self,
        _key: &str,
        _ttl: std::time::Duration,
        _signer: &dyn LinkSigning,
    ) -> Option<(String, chrono::DateTime<chrono::Utc>)> {
        None
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

/// Default key prefix for 3D outputs. Distinct from images' `litegen/images` so
/// a shared bucket keeps the two modalities apart.
pub const MODEL3D_PATH_PREFIX: &str = "litegen/3d";

/// Default key prefix for re-hosted videos.
pub const VIDEO_PATH_PREFIX: &str = "litegen/video";

/// Default key prefix for kept image results.
pub const IMAGE_RESULT_PATH_PREFIX: &str = "litegen/image-results";

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

/// The extension, and the `format` an asset reports, for an image of
/// `content_type`: png, jpg, webp, gif or svg, else png. Parameters
/// (`; charset=…`) and case are ignored.
pub fn image_format(content_type: &str) -> &'static str {
    match content_type.split(';').next().unwrap_or("").trim().to_ascii_lowercase().as_str() {
        "image/jpeg" | "image/jpg" => "jpg",
        "image/webp" => "webp",
        "image/gif" => "gif",
        "image/svg+xml" => "svg",
        _ => "png",
    }
}

/// The content type to store a 3D asset under.
///
/// Derived from the container rather than taken from the adapter, so two
/// adapters cannot tag the same format differently — the browser viewer and any
/// CDN in front of the bucket both key off this. The provider's own value is
/// the fallback for formats not listed here (textures and previews, mostly).
///
/// `fbx` and `ply` are deliberately absent: neither has an IANA registration,
/// and inventing one would be worse than `application/octet-stream`. `usdc` is
/// the same case, but listed: the IANA model registry (`model.csv`, checked
/// 2026-11-10) holds `vnd.usda` and `vnd.usdz+zip` and no type for a binary
/// USD crate, so it is `application/octet-stream` whatever the provider says.
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
        "usdc" => "application/octet-stream",
        "zip" => "application/zip",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "webp" => "image/webp",
        "gif" => "image/gif",
        "svg" => "image/svg+xml",
        "mp4" => "video/mp4",
        "webm" => "video/webm",
        "mov" => "video/quicktime",
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
/// `proxy::storage::sweep_expired` deletes what is older than the configured retention.
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
    /// The file streamed from disk, [`STREAM_CHUNK`] at a time; a range is
    /// read by seeking, so only its bytes are read.
    async fn open(&self, key: &str, range: Option<ByteRange>) -> Result<Option<StoredFile>, ImageStoreError> {
        use futures::StreamExt;
        use tokio::io::{AsyncReadExt, AsyncSeekExt};
        let failed = |e: std::io::Error| ImageStoreError::UploadFailed(format!("local read failed: {e}"));
        let path = self.path_for(key)?;
        let mut file = match tokio::fs::File::open(&path).await {
            Ok(f) => f,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
            Err(e) => return Err(failed(e)),
        };
        let meta = file.metadata().await.map_err(failed)?;
        if !meta.is_file() {
            return Err(failed(std::io::Error::other("not a regular file")));
        }
        let len = meta.len();
        let ext = key.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
        let content_type = model3d_content_type(ext, "application/octet-stream").to_string();
        let (served, start, count) = match range.map(|r| r.resolve(len)) {
            None => (Served::Whole, 0, len),
            Some(Some((start, end))) => (Served::Part { start, end }, start, end - start + 1),
            Some(None) => {
                let body = futures::stream::empty().boxed();
                return Ok(Some(StoredFile { content_type, len: Some(len), served: Served::Unsatisfiable, accepts_ranges: true, body }));
            }
        };
        if start > 0 {
            file.seek(std::io::SeekFrom::Start(start)).await.map_err(failed)?;
        }
        let body = read_chunks(file.take(count));
        Ok(Some(StoredFile { content_type, len: Some(len), served, accepts_ranges: true, body }))
    }
    /// Moved into place by a rename (the spool is beside the files, on the
    /// same filesystem), else copied: a reader never sees a half-written file.
    async fn put_file(&self, key: &str, path: &std::path::Path, _content_type: &str) -> Result<String, ImageStoreError> {
        let failed = |e: std::io::Error| ImageStoreError::UploadFailed(format!("local write failed: {e}"));
        let target = self.path_for(key)?;
        if let Some(dir) = target.parent() {
            tokio::fs::create_dir_all(dir).await.map_err(failed)?;
        }
        if tokio::fs::rename(path, &target).await.is_err() {
            // Another filesystem: copied aside, then renamed into place.
            let tmp = target.with_extension(format!("tmp-{}", uuid::Uuid::new_v4().simple()));
            tokio::fs::copy(path, &tmp).await.map_err(failed)?;
            tokio::fs::rename(&tmp, &target).await.map_err(failed)?;
        }
        Ok(format!("{}/v1/models3d/assets/{}", self.public_base_url, key))
    }
    /// `<root>/litegen/.spool`: beside the files, never served (the asset
    /// route serves only result prefixes), and swept with them.
    fn spool_dir(&self) -> Option<std::path::PathBuf> {
        Some(self.root.join("litegen").join(".spool"))
    }
    fn key_of(&self, url: &str) -> Option<String> {
        url.strip_prefix(&format!("{}/v1/models3d/assets/", self.public_base_url))
            .filter(|k| self.path_for(k).is_ok())
            .map(str::to_string)
    }
    /// litegen's own asset route with an expiry and an HMAC over the key and
    /// that expiry, which the route checks before serving.
    async fn signed_url(
        &self,
        key: &str,
        ttl: std::time::Duration,
        signer: &dyn LinkSigning,
    ) -> Option<(String, chrono::DateTime<chrono::Utc>)> {
        self.path_for(key).ok()?;
        let expires = chrono::Utc::now() + chrono::Duration::seconds(i64::try_from(ttl.as_secs()).ok()?);
        let expires = chrono::DateTime::from_timestamp(expires.timestamp(), 0)?;
        // Issued now, so a revocation (docs/storage-access.md) can end it.
        let query = signer.local_query(key, chrono::Utc::now().timestamp_millis(), expires.timestamp());
        Some((format!("{}/v1/models3d/assets/{key}?{query}", self.public_base_url), expires))
    }
}
