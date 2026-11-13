use std::sync::Arc;
use tracing::{error, info};

use crate::config::ImageStorageConfig;

// The store traits, local stores, key layout and content types are the
// engine's (src/store, engine boundary D2); this file keeps S3, bucket checks
// and building stores from config, and keeps the old paths working.
pub use crate::store::*;

/// S3-compatible object storage backend.
/// Works with AWS S3, MinIO, Cloudflare R2, DigitalOcean Spaces, etc.
pub struct S3Store {
    bucket: s3::Bucket,
    path_prefix: String,
    custom_public_url: Option<String>,
    /// Set by [`S3Store::guarded`] (see [`S3Storage::guarded`]): images go
    /// up through the guarded [`upload_client`].
    guarded: bool,
}

impl S3Store {
    pub fn from_config(config: &ImageStorageConfig) -> Result<Self, ImageStoreError> {
        let s3_cfg = config.s3.as_ref().ok_or(ImageStoreError::NotConfigured)?;

        let credentials = s3::creds::Credentials::new(
            s3_cfg.access_key_id.as_deref(),
            s3_cfg.secret_access_key.as_deref(),
            None, // security token
            s3_cfg.session_token.as_deref(),
            None, // profile
        )
        .map_err(|e| ImageStoreError::UploadFailed(format!("Invalid credentials: {e}")))?;

        // The same builder `s3_base_url` uses, so the URL a tenant's bucket is
        // checked at is the one its requests go to.
        let bucket = s3_bucket(&s3_cfg.bucket_name, &s3_cfg.region, s3_cfg.endpoint_url.as_deref(), credentials)?;

        let path_prefix = config
            .path_prefix
            .clone()
            .unwrap_or_else(|| "litegen/images".to_string());

        Ok(Self {
            bucket: *bucket,
            path_prefix,
            custom_public_url: s3_cfg.custom_public_url.clone(),
            guarded: false,
        })
    }

    /// Send this store's requests guarded (see [`S3Storage::guarded`]).
    pub fn guarded(mut self) -> Self {
        self.guarded = true;
        self
    }
}

/// How long a presigned request stays valid. Only its start is judged, so a
/// long upload still completes.
const PRESIGN_SECS: u32 = 300;

/// The client a guarded bucket's reads and deletes go through: no
/// redirects, no proxy, and only public addresses, judged at every connect
/// ([`crate::util::ssrf::public_only_download_client`]). One for the
/// process: its resolver holds no state, its pool is keyed by each URL's
/// host, and credentials travel in each presigned request. A read that
/// keeps moving runs up to an hour (a large video on a slow link); one that
/// stalls for 60 s is cut off. Uploads go through [`upload_client`]
/// instead: this client's read timeout runs until the bucket answers, which
/// for a PUT comes after the last byte, so it would cut off any upload that
/// takes over 60 s.
fn guarded_client() -> reqwest::Client {
    static CLIENT: once_cell::sync::Lazy<reqwest::Client> = once_cell::sync::Lazy::new(|| {
        crate::util::ssrf::public_only_download_client(std::time::Duration::from_secs(3600), std::time::Duration::from_secs(60))
    });
    CLIENT.clone()
}

/// The most a guarded GET reads into memory, and the most a streamed read
/// ([`S3Storage::open`]) passes on: no file litegen keeps is larger. A
/// tenant runs the server on the other end, so an endless body must not be
/// read to the end.
const MAX_STORED_FILE_BYTES: u64 = 1024 * 1024 * 1024;

/// `request`, sent with the Host its signature covers: `Bucket::host()`,
/// which keeps a default port the endpoint spells out, where a parsed URL
/// would drop it.
async fn send_signed(bucket: &s3::Bucket, request: reqwest::RequestBuilder) -> Result<reqwest::Response, ImageStoreError> {
    request
        .header(reqwest::header::HOST, bucket.host())
        .send()
        .await
        .map_err(|e| ImageStoreError::UploadFailed(format!("S3 request failed: {}", e.without_url())))
}

/// PUT `bytes` at `key` with a presigned URL through `client`
/// ([`upload_client`]), cut off once it stops moving for `idle`
/// ([`put_watched`]). Sent [`STREAM_CHUNK`] at a time, so the watch sees it
/// move; the chunks share `bytes`, nothing is copied.
async fn guarded_put(
    client: &reqwest::Client,
    bucket: &s3::Bucket,
    key: &str,
    bytes: bytes::Bytes,
    content_type: &str,
    idle: std::time::Duration,
) -> Result<(), ImageStoreError> {
    use futures::StreamExt;
    let url = bucket
        .presign_put(format!("/{key}"), PRESIGN_SECS, None, None)
        .await
        .map_err(|e| ImageStoreError::UploadFailed(format!("S3 PUT could not be signed: {e}")))?;
    let len = bytes.len();
    let chunks = (0..len).step_by(STREAM_CHUNK).map(move |at| Ok::<_, std::io::Error>(bytes.slice(at..len.min(at + STREAM_CHUNK))));
    put_watched(client, bucket, url, content_type, len as u64, futures::stream::iter(chunks).boxed(), idle).await
}

/// GET `key` with a presigned URL, through `client`: its bytes and type, or
/// `None` when there's nothing there.
async fn guarded_get(client: &reqwest::Client, bucket: &s3::Bucket, key: &str) -> Result<Option<(bytes::Bytes, String)>, ImageStoreError> {
    let url = bucket
        .presign_get(format!("/{key}"), PRESIGN_SECS, None)
        .await
        .map_err(|e| ImageStoreError::UploadFailed(format!("S3 GET could not be signed: {e}")))?;
    let resp = send_signed(bucket, client.get(url)).await?;
    match resp.status().as_u16() {
        200 => {
            let ext = key.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
            let content_type = resp
                .headers()
                .get(reqwest::header::CONTENT_TYPE)
                .and_then(|v| v.to_str().ok())
                .map(str::to_string)
                .unwrap_or_else(|| model3d_content_type(ext, "application/octet-stream").to_string());
            if resp.content_length().is_some_and(|n| n > MAX_STORED_FILE_BYTES) {
                return Err(ImageStoreError::UploadFailed("S3 GET: the object is larger than any file litegen keeps".into()));
            }
            let mut resp = resp;
            let mut bytes = Vec::new();
            while let Some(chunk) = resp.chunk().await.map_err(|e| ImageStoreError::UploadFailed(format!("S3 GET failed: {}", e.without_url())))? {
                if (bytes.len() + chunk.len()) as u64 > MAX_STORED_FILE_BYTES {
                    return Err(ImageStoreError::UploadFailed("S3 GET: the object is larger than any file litegen keeps".into()));
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(Some((bytes::Bytes::from(bytes), content_type)))
        }
        404 => Ok(None),
        code => Err(ImageStoreError::UploadFailed(format!("S3 GET returned HTTP {code}"))),
    }
}

/// The client the operator's own bucket is read through when litegen
/// streams a file from it ([`S3Storage::open`]): one for the process, no
/// redirects (a presigned request is signed for its host), the
/// environment's proxy as rust-s3's own client, and the guarded client's
/// transfer limits. Not address-guarded: the operator's bucket may sit on
/// a private address.
fn operator_bucket_client() -> reqwest::Client {
    static CLIENT: once_cell::sync::Lazy<reqwest::Client> = once_cell::sync::Lazy::new(|| {
        crate::util::ssrf::no_redirect_download_client(std::time::Duration::from_secs(3600), std::time::Duration::from_secs(60))
    });
    CLIENT.clone()
}

/// How long an upload into a bucket may go without moving before it is cut
/// off ([`put_watched`]): no chunk of its body taken for this long, or no
/// answer this long after the last chunk.
const UPLOAD_IDLE: std::time::Duration = std::time::Duration::from_secs(60);

/// The longest an upload into a bucket may take in all (1 GiB at about
/// 300 KB/s).
const UPLOAD_TOTAL: std::time::Duration = std::time::Duration::from_secs(3600);

/// The client a PUT into a bucket goes through: for a tenant's bucket
/// (`guarded`), public addresses only and no proxy; for the operator's, the
/// environment's proxy. Neither follows redirects, and neither has a read
/// timeout: reqwest's runs from sending a request until its answer's head
/// arrives, and a bucket answers a PUT only after its last byte, so it would
/// cap the whole upload. [`put_watched`] cuts off an upload that stops
/// moving instead.
fn upload_client(guarded: bool) -> reqwest::Client {
    if guarded {
        crate::util::ssrf::public_only_upload_client(UPLOAD_TOTAL)
    } else {
        crate::util::ssrf::no_redirect_upload_client(UPLOAD_TOTAL)
    }
}

/// When an upload last moved. hyper takes the next chunk of a request body
/// only once its write buffer has room for it, so a chunk taken means the
/// bucket is reading.
struct UploadProgress {
    start: tokio::time::Instant,
    /// Milliseconds from `start` to the last chunk taken.
    last: std::sync::atomic::AtomicU64,
}

impl UploadProgress {
    fn moved(&self) {
        let at = u64::try_from(self.start.elapsed().as_millis()).unwrap_or(u64::MAX);
        self.last.fetch_max(at, std::sync::atomic::Ordering::Relaxed);
    }

    /// Ready once the upload has not moved for `idle`.
    async fn stalled(&self, idle: std::time::Duration) {
        loop {
            let last = std::time::Duration::from_millis(self.last.load(std::sync::atomic::Ordering::Relaxed));
            let due = self.start + last + idle;
            if tokio::time::Instant::now() >= due {
                return;
            }
            tokio::time::sleep_until(due).await;
        }
    }
}

/// PUT `body` (`len` bytes) at the presigned `url` through `client`, with
/// its length: S3 refuses a chunked PUT. Cut off once it has not moved for
/// `idle`: no chunk of the body taken for that long, or no answer that long
/// after the last chunk. An upload that keeps moving runs up to
/// [`UPLOAD_TOTAL`].
async fn put_watched(
    client: &reqwest::Client,
    bucket: &s3::Bucket,
    url: String,
    content_type: &str,
    len: u64,
    body: futures::stream::BoxStream<'static, std::io::Result<bytes::Bytes>>,
    idle: std::time::Duration,
) -> Result<(), ImageStoreError> {
    use futures::StreamExt;
    let progress = std::sync::Arc::new(UploadProgress { start: tokio::time::Instant::now(), last: Default::default() });
    let seen = progress.clone();
    let body = body.inspect(move |_| seen.moved());
    let request = client
        .put(url)
        .header(reqwest::header::CONTENT_TYPE, content_type)
        .header(reqwest::header::CONTENT_LENGTH, len)
        .body(reqwest::Body::wrap_stream(body));
    let resp = tokio::select! {
        resp = send_signed(bucket, request) => resp?,
        () = progress.stalled(idle) => {
            return Err(ImageStoreError::UploadFailed(format!("S3 PUT stalled: the upload did not move for {} s", idle.as_secs())));
        }
    };
    if !resp.status().is_success() {
        return Err(ImageStoreError::UploadFailed(format!("S3 returned HTTP {}", resp.status().as_u16())));
    }
    Ok(())
}

/// The whole object's length a `Content-Range` names (`bytes a-b/len`, or
/// `bytes */len` on a 416), when it names one.
fn content_range_len(resp: &reqwest::Response) -> Option<u64> {
    resp.headers().get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?.rsplit_once('/')?.1.trim().parse().ok()
}

/// The first and last byte a 206's `Content-Range` names.
fn content_range_span(resp: &reqwest::Response) -> Option<(u64, u64)> {
    let value = resp.headers().get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?;
    let (start, end) = value.trim().strip_prefix("bytes ")?.split_once('/')?.0.split_once('-')?;
    Some((start.trim().parse().ok()?, end.trim().parse().ok()?))
}

/// GET `key` with a presigned URL through `client`, as a stream: the
/// bytes go on to the caller as they arrive. A range travels as a `Range`
/// header, which a presigned GET leaves unsigned (it signs only the host).
/// Counted as it streams, so a body past [`MAX_STORED_FILE_BYTES`] is cut
/// off; a declared length past it is refused before reading.
async fn presigned_open(client: &reqwest::Client, bucket: &s3::Bucket, key: &str, range: Option<ByteRange>) -> Result<Option<StoredFile>, ImageStoreError> {
    use futures::StreamExt;
    let url = bucket
        .presign_get(format!("/{key}"), PRESIGN_SECS, None)
        .await
        .map_err(|e| ImageStoreError::UploadFailed(format!("S3 GET could not be signed: {e}")))?;
    let mut request = client.get(url);
    if let Some(range) = range {
        request = request.header(reqwest::header::RANGE, range.header_value());
    }
    let resp = send_signed(bucket, request).await?;
    let ext = key.rsplit_once('.').map(|(_, e)| e).unwrap_or("");
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .map(str::to_string)
        .unwrap_or_else(|| model3d_content_type(ext, "application/octet-stream").to_string());
    let (served, len) = match resp.status().as_u16() {
        200 => (Served::Whole, resp.content_length()),
        206 => {
            let Some((start, end)) = content_range_span(&resp) else {
                return Err(ImageStoreError::UploadFailed("S3 GET: a partial answer without a Content-Range".into()));
            };
            (Served::Part { start, end }, content_range_len(&resp))
        }
        416 => {
            let body = futures::stream::empty().boxed();
            return Ok(Some(StoredFile { content_type, len: content_range_len(&resp), served: Served::Unsatisfiable, accepts_ranges: true, body }));
        }
        404 => return Ok(None),
        code => return Err(ImageStoreError::UploadFailed(format!("S3 GET returned HTTP {code}"))),
    };
    let too_large = "S3 GET: the object is larger than any file litegen keeps";
    if resp.content_length().is_some_and(|n| n > MAX_STORED_FILE_BYTES) {
        return Err(ImageStoreError::UploadFailed(too_large.into()));
    }
    let mut seen: u64 = 0;
    let body = resp
        .bytes_stream()
        .map(move |chunk| {
            let chunk = chunk.map_err(|e| std::io::Error::other(e.without_url()))?;
            seen += chunk.len() as u64;
            if seen > MAX_STORED_FILE_BYTES {
                return Err(std::io::Error::other(too_large));
            }
            Ok(chunk)
        })
        .boxed();
    Ok(Some(StoredFile { content_type, len, served, accepts_ranges: true, body }))
}

/// PUT the file at `path` at `key` with a presigned URL through `client`
/// ([`upload_client`]), streamed from disk with its length and cut off once
/// it stops moving for `idle` ([`put_watched`]). One PUT carries up to 5 GiB
/// (litegen keeps at most 1 GiB). Never rust-s3's `put_object_stream`, which
/// holds every part in memory before it sends any.
async fn presigned_put_file(
    client: &reqwest::Client,
    bucket: &s3::Bucket,
    key: &str,
    path: &std::path::Path,
    content_type: &str,
    idle: std::time::Duration,
) -> Result<(), ImageStoreError> {
    let unread = |e: std::io::Error| ImageStoreError::UploadFailed(format!("could not read {}: {e}", path.display()));
    let url = bucket
        .presign_put(format!("/{key}"), PRESIGN_SECS, None, None)
        .await
        .map_err(|e| ImageStoreError::UploadFailed(format!("S3 PUT could not be signed: {e}")))?;
    let file = tokio::fs::File::open(path).await.map_err(unread)?;
    let len = file.metadata().await.map_err(unread)?.len();
    put_watched(client, bucket, url, content_type, len, crate::store::read_chunks(file), idle).await
}

/// DELETE `key` with a presigned URL, through `client`. Gone already (404)
/// counts as deleted; any other refusal is an error, so nothing is marked
/// deleted that is still there.
async fn guarded_delete(client: &reqwest::Client, bucket: &s3::Bucket, key: &str) -> Result<(), ImageStoreError> {
    let url = bucket
        .presign_delete(format!("/{key}"), PRESIGN_SECS)
        .await
        .map_err(|e| ImageStoreError::UploadFailed(format!("S3 DELETE could not be signed: {e}")))?;
    let resp = send_signed(bucket, client.delete(url)).await?;
    let status = resp.status();
    if status.is_success() || status == reqwest::StatusCode::NOT_FOUND {
        Ok(())
    } else {
        Err(ImageStoreError::UploadFailed(format!("S3 DELETE returned HTTP {}", status.as_u16())))
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
        let ext = image_format(content_type);

        let key = format!("{}/{}.{}", self.path_prefix, generation_id, ext);

        if self.guarded {
            guarded_put(&upload_client(true), &self.bucket, &key, bytes::Bytes::copy_from_slice(data), content_type, UPLOAD_IDLE).await?;
            info!(key = %key, "Image uploaded to S3");
            return Ok(self.url_for(&key));
        }

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

        info!(key = %key, "Image uploaded to S3");
        Ok(self.url_for(&key))
    }
}

impl S3Store {
    /// The absolute URL an image stored at `key` is served from: the same URL
    /// [`S3Storage`] mints for that key, so its `key_of` finds the image again.
    pub fn url_for(&self, key: &str) -> String {
        object_url(&self.bucket, self.custom_public_url.as_deref(), key)
    }
}

/// The absolute URL an object at `key` is served from.
///
/// For an S3-compatible endpoint (path-style) this is the bucket's own URL,
/// `scheme://endpoint-host/bucket/key`: the region's `Display` is only its
/// name for a custom endpoint, which used to yield a relative
/// `region/bucket/key` that no client can fetch.
fn object_url(bucket: &s3::Bucket, custom_public_url: Option<&str>, key: &str) -> String {
    if let Some(base) = custom_public_url {
        format!("{}/{}", base.trim_end_matches('/'), key)
    } else if bucket.is_path_style() {
        format!("{}/{}", bucket.url().trim_end_matches('/'), key)
    } else {
        format!("https://{}.s3.{}.amazonaws.com/{}", bucket.name(), bucket.region(), key)
    }
}

/// Why a bucket a tenant named may not be used (see [`check_tenant_bucket`]).
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TenantBucketError {
    /// `bucket_name` holds a character that could move the requests to
    /// another host.
    #[error("{0}")]
    Bucket(String),
    /// `region` holds stray characters, or names no region litegen knows
    /// while no `endpoint_url` says where the store is.
    #[error("{0}")]
    Region(String),
    /// The requests would go to an address inside the deployment, or
    /// `endpoint_url` is not a usable http(s) URL.
    #[error("{0}")]
    Endpoint(String),
    /// The host did not resolve just now; asking again later may work.
    #[error("{0}")]
    Lookup(String),
}

/// The region rust-s3 sends a bucket's requests by: a custom one on
/// `endpoint_url` when there is one, else the named region. A name rust-s3
/// doesn't know becomes the host itself (`aws-region`'s `FromStr`).
fn s3_region(region: &str, endpoint_url: Option<&str>) -> Result<s3::Region, ImageStoreError> {
    match endpoint_url {
        Some(endpoint) => Ok(s3::Region::Custom { region: region.to_string(), endpoint: endpoint.to_string() }),
        None => region.parse::<s3::Region>().map_err(|e| ImageStoreError::UploadFailed(format!("Invalid region: {e}"))),
    }
}

/// The handle litegen reaches a bucket through: path-style on a custom
/// endpoint, as S3-compatible stores expect.
fn s3_bucket(
    bucket_name: &str,
    region: &str,
    endpoint_url: Option<&str>,
    credentials: s3::creds::Credentials,
) -> Result<Box<s3::Bucket>, ImageStoreError> {
    let bucket = s3::Bucket::new(bucket_name, s3_region(region, endpoint_url)?, credentials)
        .map_err(|e| ImageStoreError::UploadFailed(format!("Failed to create bucket: {e}")))?;
    Ok(if endpoint_url.is_some() { bucket.with_path_style() } else { bucket })
}

/// The base URL rust-s3 builds every request for this bucket on
/// (`scheme://host[/bucket]`). With `endpoint_url` set, that is the
/// endpoint's host with the bucket in the path. Otherwise it is
/// `{bucket}.{the region's host}`. rust-s3 reads an endpoint with no `://`
/// as https.
pub fn s3_base_url(bucket_name: &str, region: &str, endpoint_url: Option<&str>) -> Result<String, ImageStoreError> {
    let region = s3_region(region, endpoint_url)?;
    // `Bucket::url()` (rust-s3 0.38), without building a whole `Bucket` and its
    // HTTPS client for each check: path-style on a custom endpoint, as
    // `s3_bucket` sets it, else the bucket in the host.
    // `the_base_url_matches_a_built_bucket` holds the two together.
    Ok(if endpoint_url.is_some() {
        format!("{}://{}/{}", region.scheme(), region.host(), bucket_name)
    } else {
        format!("{}://{}.{}", region.scheme(), bucket_name, region.host())
    })
}

/// Hosted mode's rule for the fields of a bucket a tenant names
/// (`PUT /v1/apps/{app_id}/storage`): none of them may steer litegen's
/// requests anywhere but the host they plainly name. Returns the base URL the
/// requests would go to, which [`check_tenant_bucket`] then checks.
///
/// - `bucket_name`: letters, digits, `.`, `-` and `_`, the characters S3 and
///   the S3-compatible stores use. With no endpoint the name is part of the
///   host, so `@`, `/`, `:`, `#`, `?`, `\` or a space would move the request
///   to another host.
/// - `region`: letters, digits, `-` and `_`. With no `endpoint_url` it must be
///   a region rust-s3 knows, because an unknown name becomes the host.
/// - The resulting URL must be http(s) and carry no user name or password.
pub fn tenant_bucket_base_url(bucket_name: &str, region: &str, endpoint_url: Option<&str>) -> Result<String, TenantBucketError> {
    let plain = |s: &str, extra: &[u8], max: usize| {
        !s.is_empty() && s.len() <= max && s.bytes().all(|b| b.is_ascii_alphanumeric() || extra.contains(&b))
    };
    if !plain(bucket_name, b".-_", 255) {
        return Err(TenantBucketError::Bucket(format!(
            "bucket_name {bucket_name:?} may hold only letters, digits, '.', '-' and '_'"
        )));
    }
    if !plain(region, b"-_", 64) {
        return Err(TenantBucketError::Region(format!("region {region:?} may hold only letters, digits, '-' and '_'")));
    }
    if endpoint_url.is_none() && matches!(region.parse::<s3::Region>(), Ok(s3::Region::Custom { .. })) {
        return Err(TenantBucketError::Region(format!(
            "region {region:?} is not a region litegen knows; set endpoint_url to the store's URL"
        )));
    }
    let base = s3_base_url(bucket_name, region, endpoint_url).map_err(|e| TenantBucketError::Endpoint(e.to_string()))?;
    let url = reqwest::Url::parse(&base)
        .map_err(|e| TenantBucketError::Endpoint(format!("the bucket's URL {base:?} is not valid: {e}")))?;
    if !matches!(url.scheme(), "http" | "https") {
        return Err(TenantBucketError::Endpoint(format!("endpoint_url must be http or https, not {}", url.scheme())));
    }
    if !url.username().is_empty() || url.password().is_some() {
        return Err(TenantBucketError::Endpoint("endpoint_url may not carry a user name or password".to_string()));
    }
    Ok(base)
}

/// [`tenant_bucket_base_url`], then the address check webhooks get: the host
/// the requests go to must resolve only to public addresses
/// ([`crate::util::ssrf::check_public_url`]). This stops a tenant's bucket
/// from sending litegen's signed GET, PUT and DELETE requests inside the
/// deployment (the cloud metadata endpoint, a private service), where the
/// file route would hand back what they return.
///
/// rust-s3 builds its own HTTP client, so the approved addresses can't be
/// pinned. A name whose answer changes between this check and the connect
/// is the remaining gap, so callers check again at every use (`app_bucket`).
pub async fn check_tenant_bucket(bucket_name: &str, region: &str, endpoint_url: Option<&str>) -> Result<(), TenantBucketError> {
    let base = tenant_bucket_base_url(bucket_name, region, endpoint_url)?;
    crate::util::ssrf::check_public_url(&base).await.map_err(|e| match e {
        crate::util::ssrf::UrlCheckError::Lookup(why) => TenantBucketError::Lookup(why),
        crate::util::ssrf::UrlCheckError::Refused(why) => TenantBucketError::Endpoint(why),
    })
}

/// S3Storage: delegates to the S3 bucket.
pub struct S3Storage {
    bucket: s3::Bucket,
    custom_public_url: Option<String>,
    /// When its credentials stop working (an assumed role's session).
    credentials_expire_at: Option<chrono::DateTime<chrono::Utc>>,
    /// Set by [`S3Storage::guarded`]: reads and deletes go out presigned
    /// through this client instead of rust-s3's own, and uploads through the
    /// guarded [`upload_client`].
    guard: Option<reqwest::Client>,
    /// How long an upload may go without moving: [`UPLOAD_IDLE`], shorter
    /// in tests.
    upload_idle: std::time::Duration,
}

impl S3Storage {
    pub fn from_config(config: &ImageStorageConfig) -> Result<Self, ImageStoreError> {
        let s3_cfg = config.s3.as_ref().ok_or(ImageStoreError::NotConfigured)?;
        let credentials = s3::creds::Credentials::new(
            s3_cfg.access_key_id.as_deref(),
            s3_cfg.secret_access_key.as_deref(),
            None, s3_cfg.session_token.as_deref(), None,
        ).map_err(|e| ImageStoreError::UploadFailed(format!("Invalid credentials: {e}")))?;
        // The same builder `s3_base_url` uses (see `S3Store::from_config`).
        let bucket = s3_bucket(&s3_cfg.bucket_name, &s3_cfg.region, s3_cfg.endpoint_url.as_deref(), credentials)?;
        Ok(Self {
            bucket: *bucket,
            custom_public_url: s3_cfg.custom_public_url.clone(),
            credentials_expire_at: s3_cfg.credentials_expire_at,
            guard: None,
            upload_idle: UPLOAD_IDLE,
        })
    }

    /// Test only: cut uploads off after `idle` without moving, instead of
    /// [`UPLOAD_IDLE`].
    #[cfg(test)]
    pub(crate) fn with_upload_idle(mut self, idle: std::time::Duration) -> Self {
        self.upload_idle = idle;
        self
    }

    /// Send this bucket's requests as presigned URLs through a client that
    /// connects only to public addresses: for a hosted tenant's bucket.
    /// rust-s3's own client resolves the host again when it connects, out of
    /// the address guard's sight, so a name whose answer moved inside the
    /// deployment after `check_tenant_bucket` (DNS rebinding) would reach it.
    /// Presigning makes no network call; the signed request then goes through
    /// [`crate::util::ssrf::PublicOnlyResolver`] like any caller's URL.
    pub fn guarded(mut self) -> Self {
        self.guard = Some(guarded_client());
        self
    }

    /// The absolute URL an object at `key` is served from (see [`object_url`]).
    fn object_url(&self, key: &str) -> String {
        object_url(&self.bucket, self.custom_public_url.as_deref(), key)
    }
}

#[async_trait::async_trait]
impl ImageStorage for S3Storage {
    async fn put(&self, key: &str, bytes: &bytes::Bytes, content_type: &str) -> Result<String, ImageStoreError> {
        if self.guard.is_some() {
            guarded_put(&upload_client(true), &self.bucket, key, bytes.clone(), content_type, self.upload_idle).await?;
            return Ok(self.object_url(key));
        }
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
        if let Some(client) = &self.guard {
            return guarded_delete(client, &self.bucket, key).await;
        }
        let response = self.bucket.delete_object(key).await
            .map_err(|e| ImageStoreError::UploadFailed(format!("S3 DELETE failed: {e}")))?;
        // rust-s3 is built without `fail-on-err`, so a refusal comes back as
        // a response: it is an error, or a file still there would be marked
        // deleted. Gone already (404) counts as deleted.
        match response.status_code() {
            200..=299 | 404 => Ok(()),
            code => Err(ImageStoreError::UploadFailed(format!("S3 DELETE returned HTTP {code}"))),
        }
    }
    async fn get(&self, key: &str) -> Result<Option<(bytes::Bytes, String)>, ImageStoreError> {
        if let Some(client) = &self.guard {
            return guarded_get(client, &self.bucket, key).await;
        }
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
    /// A presigned GET, streamed: through the guard for a tenant's bucket,
    /// else through [`operator_bucket_client`].
    async fn open(&self, key: &str, range: Option<ByteRange>) -> Result<Option<StoredFile>, ImageStoreError> {
        let client = self.guard.clone().unwrap_or_else(operator_bucket_client);
        presigned_open(&client, &self.bucket, key, range).await
    }
    /// A presigned PUT streamed from disk ([`presigned_put_file`]), through
    /// [`upload_client`]: guarded for a tenant's bucket.
    async fn put_file(&self, key: &str, path: &std::path::Path, content_type: &str) -> Result<String, ImageStoreError> {
        presigned_put_file(&upload_client(self.guard.is_some()), &self.bucket, key, path, content_type, self.upload_idle).await?;
        Ok(self.object_url(key))
    }
    fn key_of(&self, url: &str) -> Option<String> {
        let base = self.object_url("");
        url.strip_prefix(&base).filter(|k| !k.is_empty()).map(str::to_string)
    }
    /// A presigned GET on the bucket's own endpoint, never `custom_public_url`:
    /// a CDN in front of the bucket cannot check an S3 signature.
    async fn signed_url(
        &self,
        key: &str,
        ttl: std::time::Duration,
        _signer: &dyn crate::store::LinkSigning,
    ) -> Option<(String, chrono::DateTime<chrono::Utc>)> {
        let secs = u32::try_from(ttl.as_secs()).ok()?;
        let expires = chrono::Utc::now() + chrono::Duration::seconds(i64::from(secs));
        // A presigned link dies with the session that signed it, whatever it
        // says: past that, litegen signs the link itself.
        if self.credentials_expire_at.is_some_and(|end| end < expires) {
            return None;
        }
        match self.bucket.presign_get(format!("/{key}"), secs, None).await {
            Ok(url) => Some((url, expires)),
            Err(e) => {
                error!(key = %key, error = %e, "could not presign a stored file");
                None
            }
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

/// Delete every file litegen keeps under `root` — its own `litegen/` tree,
/// never anything else in the directory — last modified more than
/// `retention` before `now`, then every directory that leaves empty (never
/// `root` itself). Returns how many files were deleted. Best-effort: an entry
/// that cannot be read or removed is skipped, and the next sweep tries again.
pub fn sweep_expired(root: &std::path::Path, retention: std::time::Duration, now: std::time::SystemTime) -> usize {
    let cutoff = now.checked_sub(retention).unwrap_or(std::time::UNIX_EPOCH);
    let mut removed = 0;
    // Explicit stacks: a directory walk must not recurse on attacker-shaped depth.
    let mut dirs = vec![root.join("litegen")];
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

/// Where a kept video waits on disk between its download and a store that
/// has no place of its own (S3, an app's bucket): set at startup to the
/// asset directory's `litegen/.spool` (in the Docker image, its volume) when
/// litegen can write there, in S3 mode too; the system temp dir otherwise.
pub fn video_spool_dir() -> std::path::PathBuf {
    VIDEO_SPOOL.get().cloned().unwrap_or_else(std::env::temp_dir)
}

/// Set [`video_spool_dir`], once, at startup.
pub fn set_video_spool_dir(dir: std::path::PathBuf) {
    let _ = VIDEO_SPOOL.set(dir);
}

static VIDEO_SPOOL: std::sync::OnceLock<std::path::PathBuf> = std::sync::OnceLock::new();

/// How a spool file's name starts and ends: [`sweep_spool`] removes only these.
pub const SPOOL_FILE_PREFIX: &str = "litegen-video-";
pub const SPOOL_FILE_SUFFIX: &str = ".part";

/// Delete the spool files in `dir` (named `litegen-video-*.part`; nothing
/// else, since `dir` may be the system temp dir) last modified more than
/// `older_than` before `now`: what a stop or a crash left mid-keep. Returns
/// how many were deleted.
pub fn sweep_spool(dir: &std::path::Path, older_than: std::time::Duration, now: std::time::SystemTime) -> usize {
    let cutoff = now.checked_sub(older_than).unwrap_or(std::time::UNIX_EPOCH);
    let Ok(entries) = std::fs::read_dir(dir) else { return 0 };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let name = name.to_string_lossy();
            name.starts_with(SPOOL_FILE_PREFIX) && name.ends_with(SPOOL_FILE_SUFFIX)
        })
        .filter(|e| e.metadata().is_ok_and(|m| m.is_file() && m.modified().is_ok_and(|t| t < cutoff)))
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Whether litegen can keep files under `dir`, the local backend's asset
/// directory: it is created if missing, then written to and read back.
pub fn check_local_dir_writable(dir: &std::path::Path) -> Result<(), String> {
    let fail = |e: std::io::Error| {
        format!("litegen cannot write its asset directory {} ({e}); make it writable by the user litegen runs as, or set image_storage.local_dir", dir.display())
    };
    std::fs::create_dir_all(dir).map_err(fail)?;
    let probe = dir.join(format!(".litegen-write-check-{}", std::process::id()));
    std::fs::write(&probe, b"ok").map_err(fail)?;
    let read = std::fs::read(&probe).map_err(fail);
    let _ = std::fs::remove_file(&probe);
    read.map(|_| ())
}

/// Run [`sweep_expired`] over `root` every hour, off the async runtime.
pub fn spawn_retention_sweeper(root: std::path::PathBuf, retention_days: u32) -> tokio::task::JoinHandle<()> {
    let retention = std::time::Duration::from_secs(u64::from(retention_days) * 86_400);
    tokio::spawn(async move {
        let mut tick = tokio::time::interval(std::time::Duration::from_secs(3600));
        loop {
            tick.tick().await;
            let root = root.clone();
            match tokio::task::spawn_blocking(move || {
                // Published references go within the hour (L4); everything
                // else keeps the retention.
                crate::proxy::ref_publish::sweep_expired_refs(&root, chrono::Utc::now().timestamp())
                    + sweep_expired(&root, retention, std::time::SystemTime::now())
            })
            .await
            {
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
                session_token: None,
                credentials_expire_at: None,
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

    #[tokio::test]
    async fn an_s3_store_presigns_on_the_bucket_endpoint_with_the_ttl() {
        let signer = crate::access::signing::LinkSigner::from_secrets_key(None);
        let key = "litegen/3d/litegen-3d-abc/model.glb";
        let before = chrono::Utc::now();
        // A CDN in front of the bucket cannot check an S3 signature, so the
        // link goes to the bucket's own endpoint even when one is set.
        let store = s3(Some("http://minio:9000"), Some("https://cdn.example.com"));
        let (url, expires) = store.signed_url(key, std::time::Duration::from_secs(900), &signer).await.expect("an S3 store signs");
        assert!(url.starts_with("http://minio:9000/litegen/litegen/3d/litegen-3d-abc/model.glb?"), "{url}");
        assert!(url.contains("X-Amz-Expires=900") && url.contains("X-Amz-Signature="), "{url}");
        let secs = (expires - before).num_seconds();
        assert!((899..=901).contains(&secs), "expires {secs}s from now");
    }

    #[tokio::test]
    async fn a_local_store_signs_its_own_route() {
        let signer = crate::access::signing::LinkSigner::from_secrets_key(None);
        let dir = tempfile::tempdir().unwrap();
        let store = LocalModel3dStorage::new(dir.path(), "http://127.0.0.1:4000/".into());
        let key = "litegen/3d/litegen-3d-abc/model.glb";
        let before = chrono::Utc::now();
        let (url, expires) = store.signed_url(key, std::time::Duration::from_secs(600), &signer).await.expect("a local store signs");
        let secs = (expires - before).num_seconds();
        assert!((599..=601).contains(&secs), "expires {secs}s from now");
        let (path, query) = url.split_once('?').unwrap();
        assert_eq!(path, format!("http://127.0.0.1:4000/v1/models3d/assets/{key}"));
        let issued: i64 = query.split('&').find_map(|p| p.strip_prefix("issued=")).expect("an issue time").parse().unwrap();
        assert!((before.timestamp_millis()..=chrono::Utc::now().timestamp_millis()).contains(&issued), "issued now, in ms");
        assert_eq!(query, signer.local_query(key, issued, expires.timestamp()));
        assert!(signer.verify_local(key, Some(issued), expires.timestamp(), query.rsplit_once("signature=").unwrap().1));
        // The store still recognises its own (signed) URL's key.
        assert_eq!(store.key_of(url.split('?').next().unwrap()).as_deref(), Some(key));
        // A key that would leave the store is never signed.
        assert!(store.signed_url("litegen/../etc/passwd", std::time::Duration::from_secs(600), &signer).await.is_none());
    }

    #[test]
    fn the_image_store_mints_the_same_url_as_the_asset_store() {
        // S3Store (the image path) and S3Storage (every other file) share a
        // bucket config; a signed or private answer finds the image's key
        // through S3Storage::key_of, so their URLs must agree. Path-style
        // endpoints used to give the image store a relative `region/bucket/key`.
        for (endpoint, public) in [
            (Some("http://minio:9000"), None),
            (None, None),
            (Some("https://s3.fr-par.scw.cloud"), Some("https://cdn.example.com/")),
        ] {
            let config = ImageStorageConfig {
                backend: "s3".into(),
                s3: Some(crate::config::S3StorageConfig {
                    bucket_name: "litegen".into(),
                    region: if endpoint.is_some() { "fr-par".into() } else { "eu-west-1".into() },
                    access_key_id: Some("k".into()),
                    secret_access_key: Some("s".into()),
                    endpoint_url: endpoint.map(str::to_string),
                    custom_public_url: public.map(str::to_string),
                    session_token: None,
                    credentials_expire_at: None,
                }),
                path_prefix: None,
                ..Default::default()
            };
            let images = S3Store::from_config(&config).unwrap();
            let assets = S3Storage::from_config(&config).unwrap();
            let key = "litegen/images/2b1c.png";
            assert_eq!(images.url_for(key), assets.object_url(key), "{endpoint:?} {public:?}");
            assert!(images.url_for(key).starts_with("http"), "absolute: {}", images.url_for(key));
            assert_eq!(assets.key_of(&images.url_for(key)).as_deref(), Some(key));
        }
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

    /// Every chunk of an opened file, joined.
    async fn read_all(file: StoredFile) -> (Vec<u8>, usize) {
        use futures::StreamExt;
        let mut body = file.body;
        let (mut all, mut chunks) = (Vec::new(), 0);
        while let Some(chunk) = body.next().await {
            all.extend_from_slice(&chunk.unwrap());
            chunks += 1;
        }
        (all, chunks)
    }

    #[tokio::test]
    async fn a_local_store_opens_a_file_as_a_stream_of_its_bytes() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalModel3dStorage::new(dir.path(), "http://h".into());
        let bytes: Vec<u8> = (0..3 * STREAM_CHUNK + 5).map(|i| (i % 251) as u8).collect();
        store.put("litegen/3d/g/model.glb", &bytes::Bytes::from(bytes.clone()), "model/gltf-binary").await.unwrap();
        let file = store.open("litegen/3d/g/model.glb", None).await.unwrap().expect("found");
        assert_eq!((file.content_type.as_str(), file.len, file.served, file.accepts_ranges), ("model/gltf-binary", Some(bytes.len() as u64), Served::Whole, true));
        let (got, chunks) = read_all(file).await;
        assert_eq!(got, bytes);
        assert!(chunks >= 4, "read {STREAM_CHUNK} bytes at a time, not whole: {chunks} chunk(s)");
        assert!(store.open("litegen/3d/g/missing.glb", None).await.unwrap().is_none(), "nothing there");
    }

    #[tokio::test]
    async fn a_local_store_opens_just_the_range_asked() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalModel3dStorage::new(dir.path(), "http://h".into());
        let bytes = b"0123456789";
        store.put("litegen/3d/g/model.glb", &bytes::Bytes::from_static(bytes), "x").await.unwrap();
        for (range, served, want) in [
            (ByteRange::FromTo(2, 5), Served::Part { start: 2, end: 5 }, &b"2345"[..]),
            (ByteRange::FromTo(8, 99), Served::Part { start: 8, end: 9 }, &b"89"[..]),
            (ByteRange::From(7), Served::Part { start: 7, end: 9 }, &b"789"[..]),
            (ByteRange::Last(3), Served::Part { start: 7, end: 9 }, &b"789"[..]),
            (ByteRange::Last(99), Served::Part { start: 0, end: 9 }, &b"0123456789"[..]),
            (ByteRange::From(10), Served::Unsatisfiable, &b""[..]),
            (ByteRange::FromTo(10, 12), Served::Unsatisfiable, &b""[..]),
            (ByteRange::Last(0), Served::Unsatisfiable, &b""[..]),
        ] {
            let file = store.open("litegen/3d/g/model.glb", Some(range)).await.unwrap().expect("found");
            assert_eq!((file.served, file.len), (served, Some(10)), "{range:?}");
            assert_eq!(read_all(file).await.0, want, "{range:?}");
        }
        for key in ["../escape.glb", "a/../../b.glb", "/etc/passwd"] {
            assert!(store.open(key, Some(ByteRange::From(0))).await.is_err(), "{key:?} was opened");
        }
    }

    /// Review 2026-11-11 P4: the local store takes a spooled file by
    /// renaming it into place: nothing is read into memory or left behind.
    #[tokio::test]
    async fn a_local_store_takes_a_spooled_file_by_renaming_it() {
        let dir = tempfile::tempdir().unwrap();
        let store = LocalModel3dStorage::new(dir.path(), "http://h".into());
        let spool = store.spool_dir().expect("the local store spools beside its files");
        assert_eq!(spool, dir.path().join("litegen/.spool"));
        std::fs::create_dir_all(&spool).unwrap();
        let part = spool.join("litegen-video-x.part");
        std::fs::write(&part, b"the video").unwrap();
        let url = store.put_file("litegen/video/g/video.mp4", &part, "video/mp4").await.unwrap();
        assert_eq!(url, "http://h/v1/models3d/assets/litegen/video/g/video.mp4");
        assert_eq!(std::fs::read(dir.path().join("litegen/video/g/video.mp4")).unwrap(), b"the video");
        assert!(!part.exists(), "moved, not copied");
        assert!(store.put_file("../escape.mp4", &part, "video/mp4").await.is_err());
    }

    /// Review 2026-11-11 P4: only litegen's own spool files, and only stale
    /// ones, are swept: the spool may be the system temp dir.
    #[test]
    fn the_spool_sweep_removes_only_litegens_stale_files() {
        let dir = tempfile::tempdir().unwrap();
        for name in ["litegen-video-a.part", "someone-else.part", "litegen-video-b.txt"] {
            std::fs::write(dir.path().join(name), b"x").unwrap();
        }
        let now = std::time::SystemTime::now();
        assert_eq!(sweep_spool(dir.path(), std::time::Duration::from_secs(3600), now), 0, "nothing is stale yet");
        let later = now + std::time::Duration::from_secs(7200);
        assert_eq!(sweep_spool(dir.path(), std::time::Duration::from_secs(3600), later), 1);
        assert!(!dir.path().join("litegen-video-a.part").exists());
        assert!(dir.path().join("someone-else.part").exists() && dir.path().join("litegen-video-b.txt").exists());
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

    /// The shipped image ran as a user who could not write its asset
    /// directory, so every kept result failed at delivery, after the vendor
    /// billed it. litegen now says so at startup instead.
    #[cfg(unix)]
    #[test]
    fn an_asset_directory_litegen_cannot_write_is_named_at_startup() {
        use std::os::unix::fs::PermissionsExt;
        let dir = tempfile::tempdir().unwrap();
        assert!(check_local_dir_writable(&dir.path().join("data/assets")).is_ok(), "created and writable");
        let locked = dir.path().join("locked");
        std::fs::create_dir(&locked).unwrap();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o555)).unwrap();
        let err = check_local_dir_writable(&locked.join("assets")).unwrap_err();
        std::fs::set_permissions(&locked, std::fs::Permissions::from_mode(0o755)).unwrap();
        assert!(err.contains("assets"), "names the directory: {err}");
    }

    #[test]
    fn the_sweeper_leaves_whatever_is_not_litegens_alone() {
        let dir = tempfile::tempdir().unwrap();
        let theirs = dir.path().join("backups/litegen.db");
        std::fs::create_dir_all(theirs.parent().unwrap()).unwrap();
        std::fs::write(&theirs, b"x").unwrap();
        let now = std::time::SystemTime::now();
        std::fs::File::options().write(true).open(&theirs).unwrap().set_modified(now - std::time::Duration::from_secs(30 * 86_400)).unwrap();
        assert_eq!(sweep_expired(dir.path(), std::time::Duration::from_secs(7 * 86_400), now), 0);
        assert!(theirs.exists(), "a file litegen did not write under its asset root stays");
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

#[cfg(test)]
mod image_format_tests {
    use super::*;

    #[test]
    fn image_formats_follow_the_content_type() {
        for (content_type, format) in [
            ("image/png", "png"),
            ("image/jpeg", "jpg"),
            ("image/jpg", "jpg"),
            ("image/webp", "webp"),
            ("image/gif", "gif"),
            ("image/svg+xml", "svg"),
            ("image/svg+xml; charset=utf-8", "svg"),
            ("IMAGE/GIF", "gif"),
            ("application/octet-stream", "png"),
        ] {
            assert_eq!(image_format(content_type), format, "{content_type}");
        }
        // A kept image is served back under the type it was kept as (the
        // local store reads it from the key's extension).
        for (format, content_type) in [("png", "image/png"), ("jpg", "image/jpeg"), ("webp", "image/webp"), ("gif", "image/gif"), ("svg", "image/svg+xml")] {
            assert_eq!(model3d_content_type(format, "application/octet-stream"), content_type, "{format}");
        }
    }
}

#[cfg(test)]
mod usd_content_type_tests {
    use super::*;

    /// IANA registers `model/vnd.usda` and `model/vnd.usdz+zip` and nothing for
    /// a binary USD crate, so a `usdc` goes out as `application/octet-stream`
    /// whatever type an adapter or a vendor declares for it.
    #[test]
    fn a_usdc_is_octet_stream_because_iana_registers_no_type_for_it() {
        assert_eq!(model3d_content_type("usda", "application/octet-stream"), "model/vnd.usda");
        assert_eq!(model3d_content_type("usdz", "application/octet-stream"), "model/vnd.usdz+zip");
        assert_eq!(model3d_content_type("usdc", "model/vnd.usdc"), "application/octet-stream");
        assert_eq!(model3d_content_type("usdc", "model/gltf-binary"), "application/octet-stream");
    }
}

/// Security fix 1 (2026-11-05): a bucket a hosted tenant names must not send
/// litegen's signed requests inside the deployment.
#[cfg(test)]
mod tenant_bucket_tests {
    use super::*;

    #[test]
    fn the_base_url_is_the_one_rust_s3_sends_requests_to() {
        // A custom endpoint: path-style, the bucket in the path.
        assert_eq!(s3_base_url("b", "us-east-1", Some("http://127.0.0.1:9000")).unwrap(), "http://127.0.0.1:9000/b");
        // A known region: the bucket is part of the host.
        assert_eq!(s3_base_url("b", "us-west-2", None).unwrap(), "https://b.s3-us-west-2.amazonaws.com");
        // A region rust-s3 doesn't know *is* the host, which is why a tenant's
        // region needs a rule of its own.
        assert_eq!(s3_base_url("b", "10.0.0.5:9000", None).unwrap(), "https://b.10.0.0.5:9000");
    }

    #[test]
    fn a_tenant_bucket_field_that_could_move_the_host_is_refused() {
        for bucket in ["", "x@169.254.169.254#", "a/b", "a?b", "a#b", "a:b", "a\\b", "a b", "a%2fb", "a\tb"] {
            assert!(
                matches!(tenant_bucket_base_url(bucket, "us-east-1", None), Err(TenantBucketError::Bucket(_))),
                "bucket {bucket:?}"
            );
        }
        // No endpoint: an unknown region would become the host.
        for region in ["169.254.169.254", "localhost", "10.0.0.5:9000", "metadata.google.internal"] {
            assert!(
                matches!(tenant_bucket_base_url("b", region, None), Err(TenantBucketError::Region(_))),
                "region {region:?}"
            );
        }
        // With an endpoint the region only signs, but stray characters still go.
        for region in ["", "us-east-1\r\nx", "a/b", "a b"] {
            assert!(
                matches!(tenant_bucket_base_url("b", region, Some("https://93.184.216.34")), Err(TenantBucketError::Region(_))),
                "region {region:?}"
            );
        }
        for endpoint in ["file:///etc/passwd", "ftp://93.184.216.34", "gopher://93.184.216.34", "not a url", "https://user:pw@93.184.216.34"] {
            assert!(
                matches!(tenant_bucket_base_url("b", "us-east-1", Some(endpoint)), Err(TenantBucketError::Endpoint(_))),
                "endpoint {endpoint:?}"
            );
        }
    }

    #[tokio::test]
    async fn a_tenant_bucket_inside_the_deployment_is_refused() {
        for endpoint in [
            "http://127.0.0.1:9000",
            "http://169.254.169.254",
            "http://[::1]:9000",
            "http://10.0.0.5",
            "http://0x7f.1",          // WHATWG reads this as 127.0.0.1
            "http://[::ffff:10.0.0.1]",
            "http://0.0.0.0:9000",
        ] {
            assert!(
                matches!(check_tenant_bucket("b", "us-east-1", Some(endpoint)).await, Err(TenantBucketError::Endpoint(_))),
                "endpoint {endpoint:?}"
            );
        }
        // A field refusal comes first, before any lookup.
        assert!(matches!(check_tenant_bucket("b", "169.254.169.254", None).await, Err(TenantBucketError::Region(_))));
    }

    /// `s3_base_url` must stay what rust-s3 itself builds, for every shape
    /// of input, without building a whole `Bucket` (and its HTTPS client) for
    /// each check.
    #[test]
    fn the_base_url_matches_a_built_bucket() {
        for (bucket, region, endpoint) in [
            ("b", "us-east-1", Some("http://127.0.0.1:9000")),
            ("b", "us-east-1", Some("https://minio.example.com:9443/s3")),
            ("b", "auto", Some("s3.us-west-004.backblazeb2.com")),
            ("b", "us-east-1", Some("HTTP://10.0.0.5")),
            ("b", "us-west-2", None),
            ("Legacy_Bucket.v2", "nyc3", None),
            ("b", "10.0.0.5:9000", None),
        ] {
            let built = s3_bucket(bucket, region, endpoint, s3::creds::Credentials::anonymous().unwrap()).unwrap().url();
            assert_eq!(s3_base_url(bucket, region, endpoint).unwrap(), built, "{bucket} {region} {endpoint:?}");
        }
    }

    #[tokio::test]
    async fn a_tenant_bucket_named_inside_the_deployment_is_refused() {
        // A name that resolves inside (localhost), a scheme-less endpoint
        // (rust-s3 reads it as https) and an uppercase scheme.
        for endpoint in ["http://localhost:9000", "10.0.0.5:9000", "HTTP://10.0.0.5", "127.0.0.1"] {
            assert!(
                matches!(check_tenant_bucket("b", "us-east-1", Some(endpoint)).await, Err(TenantBucketError::Endpoint(_))),
                "endpoint {endpoint:?}"
            );
        }
        // A host that doesn't resolve now is a lookup failure, not a verdict.
        assert!(matches!(check_tenant_bucket("b", "us-east-1", Some("https://litegen-bucket-test.invalid")).await, Err(TenantBucketError::Lookup(_))));
    }

    #[test]
    fn a_scheme_less_public_endpoint_is_read_as_https() {
        assert_eq!(
            tenant_bucket_base_url("b", "us-west-004", Some("s3.us-west-004.backblazeb2.com")).unwrap(),
            "https://s3.us-west-004.backblazeb2.com/b"
        );
    }

    #[tokio::test]
    async fn a_public_tenant_bucket_passes() {
        // A public IP literal needs no lookup.
        assert_eq!(check_tenant_bucket("acme-bucket", "auto", Some("https://93.184.216.34")).await, Ok(()));
        assert_eq!(check_tenant_bucket("Legacy_Bucket.v2", "us-west-004", Some("https://93.184.216.34:8443/s3")).await, Ok(()));
        // Known regions keep working without an endpoint (the lookup is the
        // async check's, so only the fields are judged here).
        assert_eq!(tenant_bucket_base_url("acme-bucket", "us-west-2", None).unwrap(), "https://acme-bucket.s3-us-west-2.amazonaws.com");
        assert_eq!(tenant_bucket_base_url("acme-bucket", "nyc3", None).unwrap(), "https://acme-bucket.nyc3.digitaloceanspaces.com");
    }
}

/// Security fix 1, round 2: a hosted tenant's bucket sends its requests
/// presigned through the guarded client, so a name whose answer moves
/// inside the deployment after `check_tenant_bucket` (DNS rebinding) reaches
/// nothing. `localhost` stands in for such a name.
#[cfg(test)]
mod guarded_bucket_tests {
    use super::*;
    use wiremock::matchers::method;
    use wiremock::{Mock, MockServer, ResponseTemplate};

    fn config(endpoint: &str) -> ImageStorageConfig {
        ImageStorageConfig {
            backend: "s3".into(),
            s3: Some(crate::config::S3StorageConfig {
                bucket_name: "b".into(),
                region: "us-east-1".into(),
                access_key_id: Some("AKIATEST".into()),
                secret_access_key: Some("secret".into()),
                endpoint_url: Some(endpoint.into()),
                custom_public_url: None,
                session_token: None,
                credentials_expire_at: None,
            }),
            ..Default::default()
        }
    }

    async fn s3_mock() -> MockServer {
        let server = MockServer::start().await;
        Mock::given(method("PUT")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(b"mesh".to_vec(), "model/gltf-binary"))
            .mount(&server)
            .await;
        Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(204)).mount(&server).await;
        server
    }

    #[tokio::test]
    async fn a_guarded_bucket_never_connects_inside_the_deployment() {
        let server = s3_mock().await;
        let by_name = format!("http://localhost:{}", server.address().port());
        let body = bytes::Bytes::from_static(b"x");

        let storage = S3Storage::from_config(&config(&by_name)).unwrap().guarded();
        assert!(storage.put("k.glb", &body, "model/gltf-binary").await.is_err(), "put");
        assert!(storage.get("k.glb").await.is_err(), "get");
        assert!(storage.delete("k.glb").await.is_err(), "delete");
        let images = S3Store::from_config(&config(&by_name)).unwrap().guarded();
        assert!(images.store(b"x", "image/png", "g1").await.is_err(), "store");
        assert_eq!(server.received_requests().await.unwrap().len(), 0, "nothing reached the address inside");

        // rust-s3's own client, the operator's, gets there.
        S3Storage::from_config(&config(&by_name)).unwrap().put("k.glb", &body, "model/gltf-binary").await.unwrap();
        assert_eq!(server.received_requests().await.unwrap().len(), 1);
    }

    #[tokio::test]
    async fn a_guarded_bucket_sends_presigned_requests() {
        let server = s3_mock().await;
        let storage = S3Storage::from_config(&config(&server.uri())).unwrap().guarded();
        let url = storage.put("litegen/3d/g/model.glb", &bytes::Bytes::from_static(b"mesh"), "model/gltf-binary").await.unwrap();
        assert_eq!(url, format!("{}/b/litegen/3d/g/model.glb", server.uri()));
        let (got, content_type) = storage.get("litegen/3d/g/model.glb").await.unwrap().expect("found");
        assert_eq!((got.as_ref(), content_type.as_str()), (b"mesh".as_ref(), "model/gltf-binary"));
        storage.delete("litegen/3d/g/model.glb").await.unwrap();
        S3Store::from_config(&config(&server.uri())).unwrap().guarded().store(b"png", "image/png", "g1").await.unwrap();

        let requests = server.received_requests().await.unwrap();
        let methods: Vec<&str> = requests.iter().map(|r| r.method.as_str()).collect();
        assert_eq!(methods, ["PUT", "GET", "DELETE", "PUT"]);
        for r in &requests {
            assert!(r.url.query().is_some_and(|q| q.contains("X-Amz-Signature=")), "presigned: {}", r.url);
            assert!(r.headers.get("authorization").is_none(), "no Authorization header: {}", r.url);
            assert_eq!(r.headers.get("host").unwrap(), &format!("127.0.0.1:{}", server.address().port()));
        }
        assert_eq!(requests[0].url.path(), "/b/litegen/3d/g/model.glb");
        assert_eq!(requests[0].headers.get("content-type").unwrap(), "model/gltf-binary");
        assert_eq!(requests[0].body, b"mesh");
    }

    #[tokio::test]
    async fn an_s3_store_streams_a_presigned_get_and_forwards_the_range() {
        use wiremock::matchers::header;
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=0-3"))
            .respond_with(ResponseTemplate::new(206).insert_header("content-range", "bytes 0-3/10").set_body_raw(b"glTF".to_vec(), "model/gltf-binary"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=10-"))
            .respond_with(ResponseTemplate::new(416).insert_header("content-range", "bytes */10"))
            .mount(&server)
            .await;
        Mock::given(method("GET"))
            .and(header("range", "bytes=-0"))
            .respond_with(ResponseTemplate::new(416))
            .mount(&server)
            .await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        let native = S3Storage::from_config(&config(&server.uri())).unwrap();
        let guarded = S3Storage::from_config(&config(&server.uri())).unwrap().guarded();
        for (name, store) in [("native", &native), ("guarded", &guarded)] {
            let part = store.open("litegen/3d/g/model.glb", Some(ByteRange::FromTo(0, 3))).await.unwrap().expect("found");
            assert_eq!((part.served, part.len, part.accepts_ranges), (Served::Part { start: 0, end: 3 }, Some(10), true), "{name}");
            assert_eq!(part.content_type, "model/gltf-binary", "{name}");
            let mut body = part.body;
            assert_eq!(futures::StreamExt::next(&mut body).await.unwrap().unwrap().as_ref(), b"glTF", "{name}");
            let past = store.open("litegen/3d/g/model.glb", Some(ByteRange::From(10))).await.unwrap().expect("found");
            assert_eq!((past.served, past.len), (Served::Unsatisfiable, Some(10)), "{name}: the length S3 named");
            let unnamed = store.open("litegen/3d/g/model.glb", Some(ByteRange::Last(0))).await.unwrap().expect("found");
            assert_eq!((unnamed.served, unnamed.len), (Served::Unsatisfiable, None), "{name}: no length made up");
            assert!(store.open("litegen/3d/g/other.glb", None).await.unwrap().is_none(), "{name}: 404 is nothing there");
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 8);
        for r in &requests {
            assert!(r.url.query().is_some_and(|q| q.contains("X-Amz-Signature=")), "presigned: {}", r.url);
            assert!(r.headers.get("authorization").is_none(), "no Authorization header: {}", r.url);
        }
    }

    /// Review 2026-11-11 P4: a file goes to S3 in one presigned PUT that
    /// carries its length (S3 refuses a chunked PUT), through either client.
    #[tokio::test]
    async fn an_s3_store_puts_a_file_with_its_length() {
        let server = MockServer::start().await;
        Mock::given(method("PUT")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("litegen-video-x.part");
        let body: Vec<u8> = (0..300_000u32).map(|i| (i % 251) as u8).collect();
        std::fs::write(&path, &body).unwrap();
        let native = S3Storage::from_config(&config(&server.uri())).unwrap();
        let guarded = S3Storage::from_config(&config(&server.uri())).unwrap().guarded();
        for store in [&native, &guarded] {
            let url = store.put_file("litegen/video/g/video.mp4", &path, "video/mp4").await.unwrap();
            assert_eq!(url, format!("{}/b/litegen/video/g/video.mp4", server.uri()));
        }
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for r in &requests {
            assert!(r.url.query().is_some_and(|q| q.contains("X-Amz-Signature=")), "presigned: {}", r.url);
            assert_eq!(r.headers.get("content-length").unwrap(), &body.len().to_string());
            assert!(r.headers.get("transfer-encoding").is_none(), "not chunked");
            assert_eq!(r.headers.get("content-type").unwrap(), "video/mp4");
            assert_eq!(r.body, body);
        }
    }

    /// Code review 2026-11-11, payloads M1: bytes go up to a tenant's bucket
    /// [`STREAM_CHUNK`] at a time, so the upload's progress can be watched,
    /// still as one PUT with their length, and arrive whole and in order (an
    /// empty body too).
    #[tokio::test]
    async fn a_guarded_store_puts_bytes_in_chunks_as_one_put_with_their_length() {
        let server = MockServer::start().await;
        Mock::given(method("PUT")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let body: Vec<u8> = (0..3 * STREAM_CHUNK + 5).map(|i| (i % 251) as u8).collect();
        let guarded = S3Storage::from_config(&config(&server.uri())).unwrap().guarded();
        guarded.put("litegen/video/g/video.mp4", &bytes::Bytes::from(body.clone()), "video/mp4").await.unwrap();
        guarded.put("litegen/video/g/empty.mp4", &bytes::Bytes::new(), "video/mp4").await.unwrap();
        let requests = server.received_requests().await.unwrap();
        assert_eq!(requests.len(), 2);
        for (r, want) in requests.iter().zip([body.as_slice(), &[]]) {
            assert_eq!(r.headers.get("content-length").unwrap(), &want.len().to_string(), "{}", r.url);
            assert!(r.headers.get("transfer-encoding").is_none(), "not chunked: {}", r.url);
            assert!(r.body == want, "{}: the body arrived whole and in order ({} of {} bytes)", r.url, r.body.len(), want.len());
        }
    }

    /// A bucket on loopback for one PUT. It reads the body at `rate` bytes a
    /// second and answers 200 once it has all of it, as S3 does: a bucket
    /// answers a PUT only after its last byte. With `stop_after`, it stops
    /// reading after that many body bytes; with `answer` false, it reads the
    /// whole body and never answers. Returns its endpoint and the body bytes
    /// it has read so far.
    async fn paced_bucket(rate: u64, stop_after: Option<u64>, answer: bool) -> (String, std::sync::Arc<std::sync::atomic::AtomicU64>) {
        use std::sync::atomic::Ordering::SeqCst;
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let endpoint = format!("http://{}", listener.local_addr().unwrap());
        let read = std::sync::Arc::new(std::sync::atomic::AtomicU64::new(0));
        let counter = read.clone();
        tokio::spawn(async move {
            let Ok((mut sock, _)) = listener.accept().await else { return };
            let mut buf = vec![0u8; 64 * 1024];
            let mut head = Vec::new();
            let body_at = loop {
                let n = sock.read(&mut buf).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                head.extend_from_slice(&buf[..n]);
                if let Some(at) = head.windows(4).position(|w| w == b"\r\n\r\n") {
                    break at + 4;
                }
            };
            let lines = String::from_utf8_lossy(&head[..body_at]).to_ascii_lowercase();
            let len: u64 = lines
                .lines()
                .find_map(|l| l.strip_prefix("content-length:"))
                .and_then(|v| v.trim().parse().ok())
                .expect("a PUT with its length");
            let mut got = (head.len() - body_at) as u64;
            counter.store(got, SeqCst);
            let start = tokio::time::Instant::now();
            while got < len {
                if stop_after.is_some_and(|stop| got >= stop) {
                    break;
                }
                let allowed = (rate as f64 * start.elapsed().as_secs_f64()) as u64;
                if got >= allowed {
                    tokio::time::sleep(std::time::Duration::from_millis(5)).await;
                    continue;
                }
                let want = (allowed - got).min(buf.len() as u64) as usize;
                let n = sock.read(&mut buf[..want]).await.unwrap_or(0);
                if n == 0 {
                    return;
                }
                got += n as u64;
                counter.store(got, SeqCst);
            }
            if got == len && answer {
                let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Length: 0\r\n\r\n").await;
            }
            // Hold the connection, reading nothing more.
            tokio::time::sleep(std::time::Duration::from_secs(60)).await;
        });
        (endpoint, read)
    }

    /// The three ways a kept file goes up to a bucket through litegen's own
    /// client: a spool file (a video named by a link) to the operator's
    /// bucket and to a tenant's (guarded) one, and bytes in memory (a video
    /// handed over whole) to a tenant's.
    const UPLOAD_PATHS: [(&str, bool, bool); 3] =
        [("operator, from a file", false, true), ("guarded, from a file", true, true), ("guarded, from memory", true, false)];

    /// One upload's path name, outcome, time taken and the body bytes its
    /// bucket read.
    type UploadOutcome = (&'static str, Result<(), ImageStoreError>, std::time::Duration, u64);

    /// Upload `bytes` (also at `path`) to a fresh [`paced_bucket`] along each
    /// of [`UPLOAD_PATHS`] at once, with `idle` as the store's idle limit.
    async fn upload_along_each_path(
        path: &std::path::Path,
        bytes: &bytes::Bytes,
        idle: std::time::Duration,
        bucket: (u64, Option<u64>, bool),
    ) -> Vec<UploadOutcome> {
        let mut uploads = Vec::new();
        for (name, guarded, from_file) in UPLOAD_PATHS {
            let (endpoint, read) = paced_bucket(bucket.0, bucket.1, bucket.2).await;
            let store = S3Storage::from_config(&config(&endpoint)).unwrap();
            let store = if guarded { store.guarded() } else { store }.with_upload_idle(idle);
            let (path, bytes) = (path.to_path_buf(), bytes.clone());
            uploads.push(async move {
                let start = std::time::Instant::now();
                let got = if from_file {
                    store.put_file("litegen/video/g/video.mp4", &path, "video/mp4").await
                } else {
                    store.put("litegen/video/g/video.mp4", &bytes, "video/mp4").await
                };
                (name, got.map(|_| ()), start.elapsed(), read.load(std::sync::atomic::Ordering::SeqCst))
            });
        }
        futures::future::join_all(uploads).await
    }

    /// Code review 2026-11-11, payloads M1: an upload into a bucket that
    /// keeps moving is not cut off, however long it takes (up to the hour).
    /// A bucket answers a PUT only after its last byte, and reqwest's read
    /// timeout runs from sending the request until that answer, so the
    /// "silent for 60 s" limit used to cut off every upload that took over
    /// 60 s. Here the limit is 2 s, and the bucket reads each body steadily
    /// for about 4 s.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_upload_that_keeps_moving_is_not_cut_off_by_the_idle_limit() {
        const RATE: u64 = 8 * 1024 * 1024;
        let (idle, size) = (std::time::Duration::from_secs(2), 4 * RATE);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("litegen-video-x.part");
        std::fs::write(&path, vec![7u8; size as usize]).unwrap();
        let bytes = bytes::Bytes::from(vec![7u8; size as usize]);
        let outcomes = upload_along_each_path(&path, &bytes, idle, (RATE, None, true)).await;
        let cut: Vec<String> = outcomes
            .iter()
            .filter(|(_, got, _, _)| got.is_err())
            .map(|(name, got, took, read)| format!("{name}: cut off after {took:.2?}, with {read} of {size} bytes read: {got:?}"))
            .collect();
        assert!(cut.is_empty(), "an upload still moving was cut off:\n{}", cut.join("\n"));
        for (name, _, took, read) in &outcomes {
            assert_eq!(*read, size, "{name}: the bucket read the whole body");
            assert!(*took > idle, "{name}: the upload took longer than the idle limit ({took:.2?})");
        }
    }

    /// Code review 2026-11-11, payloads M1: an upload that stops moving is
    /// still cut off once it has not moved for the idle limit: a bucket that
    /// stops reading mid-body, and one that reads it all but never answers.
    /// The upload clients have no read timeout of their own, so without this
    /// such an upload would hold its slot until the hour runs out.
    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn an_upload_that_stops_moving_is_cut_off_after_the_idle_limit() {
        let (idle, size) = (std::time::Duration::from_secs(1), 8u64 * 1024 * 1024);
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("litegen-video-x.part");
        std::fs::write(&path, vec![7u8; size as usize]).unwrap();
        let bytes = bytes::Bytes::from(vec![7u8; size as usize]);
        for (bucket, stop_after, answer) in [("stops reading after 1 MiB", Some(1u64 << 20), true), ("reads it all, never answers", None, false)] {
            let outcomes = upload_along_each_path(&path, &bytes, idle, (u64::MAX, stop_after, answer)).await;
            for (name, got, took, read) in &outcomes {
                assert!(got.is_err(), "{name}, a bucket that {bucket}: the upload went through ({read} bytes read)");
                assert!(
                    *took >= idle && *took < idle + std::time::Duration::from_secs(5),
                    "{name}, a bucket that {bucket}: cut off {took:.2?} in, not about {idle:?} after it stopped moving: {got:?}"
                );
            }
        }
    }

    /// No file litegen keeps is larger than 1 GiB, so an object that says it
    /// is larger is refused before any of it is read, through either client.
    #[tokio::test]
    async fn an_s3_read_refuses_an_object_larger_than_any_file_litegen_keeps() {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut req = [0u8; 8192];
                    let _ = sock.read(&mut req).await;
                    let head = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_STORED_FILE_BYTES + 1);
                    if sock.write_all(head.as_bytes()).await.is_ok() {
                        tokio::time::sleep(std::time::Duration::from_secs(60)).await;
                    }
                });
            }
        });
        let endpoint = format!("http://{addr}");
        for store in [S3Storage::from_config(&config(&endpoint)).unwrap(), S3Storage::from_config(&config(&endpoint)).unwrap().guarded()] {
            let got = tokio::time::timeout(std::time::Duration::from_secs(10), store.open("k.glb", None)).await.expect("refused at once");
            assert!(matches!(got, Err(ImageStoreError::UploadFailed(ref m)) if m.contains("larger than any file")), "{:?}", got.map(|f| f.map(|f| f.len)));
        }
    }

    /// Review of security fix 1: a refused DELETE is an error on the
    /// operator's (native) path too, so nothing is marked deleted that is
    /// still there. rust-s3 is built without `fail-on-err`, so a refusal comes
    /// back as a response.
    #[tokio::test]
    async fn a_refused_delete_is_an_error_on_the_native_path_too() {
        for (status, deleted) in [(204u16, true), (404, true), (403, false), (500, false)] {
            let server = MockServer::start().await;
            Mock::given(method("DELETE")).respond_with(ResponseTemplate::new(status)).mount(&server).await;
            let native = S3Storage::from_config(&config(&server.uri())).unwrap();
            assert_eq!(native.delete("k.glb").await.is_ok(), deleted, "native, HTTP {status}");
            let guarded = S3Storage::from_config(&config(&server.uri())).unwrap().guarded();
            assert_eq!(guarded.delete("k.glb").await.is_ok(), deleted, "guarded, HTTP {status}");
        }
    }

    /// Against a real S3-compatible server, which checks the presigned
    /// signatures that wiremock doesn't. Gated on `LITEGEN_S3_TEST_ENDPOINT`,
    /// as tests/output_access_s3.rs is (a throwaway MinIO).
    #[tokio::test]
    async fn a_guarded_bucket_round_trips_against_a_real_s3() {
        let Ok(endpoint) = std::env::var("LITEGEN_S3_TEST_ENDPOINT") else {
            eprintln!("skipping a_guarded_bucket_round_trips_against_a_real_s3: set LITEGEN_S3_TEST_ENDPOINT");
            return;
        };
        let key = std::env::var("LITEGEN_S3_TEST_KEY").unwrap_or_else(|_| "minioadmin".into());
        let secret = std::env::var("LITEGEN_S3_TEST_SECRET").unwrap_or_else(|_| "minioadmin".into());
        let name = format!("litegen-guard-{}", uuid::Uuid::new_v4().simple());
        let region = s3::Region::Custom { region: "us-east-1".into(), endpoint: endpoint.clone() };
        let creds = s3::creds::Credentials::new(Some(&key), Some(&secret), None, None, None).unwrap();
        let created = s3::Bucket::create_with_path_style(&name, region, creds, s3::BucketConfiguration::default())
            .await
            .expect("create the test bucket");
        assert!(created.success(), "create bucket: {}", created.response_text);
        let mut cfg = config(&endpoint);
        if let Some(s3) = cfg.s3.as_mut() {
            s3.bucket_name = name.clone();
            s3.access_key_id = Some(key);
            s3.secret_access_key = Some(secret);
        }

        let storage = S3Storage::from_config(&cfg).unwrap().guarded();
        storage.put("k/model.glb", &bytes::Bytes::from_static(b"mesh"), "model/gltf-binary").await.expect("presigned PUT accepted");
        let (got, content_type) = storage.get("k/model.glb").await.expect("presigned GET accepted").expect("found");
        assert_eq!((got.as_ref(), content_type.as_str()), (b"mesh".as_ref(), "model/gltf-binary"));
        assert!(storage.get("k/missing.glb").await.expect("a miss is no error").is_none());
        storage.delete("k/model.glb").await.expect("presigned DELETE accepted");
        assert!(storage.get("k/model.glb").await.unwrap().is_none(), "deleted");

        let images = S3Store::from_config(&cfg).unwrap().guarded();
        let url = images.store(b"png", "image/png", "g1").await.expect("presigned image PUT accepted");
        let image_key = url.rsplit_once(&format!("/{name}/")).map(|(_, k)| k.to_string()).expect("the bucket's own URL");
        assert!(storage.get(&image_key).await.unwrap().is_some(), "{image_key}");
        storage.delete(&image_key).await.unwrap();
        created.bucket.delete().await.expect("remove the test bucket");
    }
}
