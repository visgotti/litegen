//! Downloading a vendor's result files: what every 3D adapter does with the
//! URLs a finished job names, so the bytes can be re-hosted before those URLs
//! expire (several vendors' do within minutes). A job that keeps nothing
//! (`keep_outputs: false`) hands out the URLs themselves, so its files are not
//! downloaded: each link is checked instead ([`probe_asset`]).

use crate::providers::ProviderError;

/// Why an asset did not download, or its link did not check out.
pub(crate) enum AssetError {
    /// Asking again may work: the connection dropped, or the host answered
    /// 5xx / 408 / 429.
    Transient(ProviderError),
    /// It never will: the URL is gone, refused, private, or too large.
    Gone(String),
}

fn transient(status: reqwest::StatusCode) -> bool {
    status.is_server_error()
        || status == reqwest::StatusCode::REQUEST_TIMEOUT
        || status == reqwest::StatusCode::TOO_MANY_REQUESTS
}

/// With `public_only`, refuse a URL that does not resolve to a public
/// address, before anything connects to it.
async fn check_public(url: &str, public_only: bool, vendor: &str) -> Result<(), AssetError> {
    if !public_only {
        return Ok(());
    }
    crate::util::ssrf::check_public_url(url).await.map_err(|why| match why {
        // A resolver blip is not a verdict on the URL.
        crate::util::ssrf::UrlCheckError::Lookup(e) => AssetError::Transient(ProviderError::RequestFailed {
            message: format!("{vendor} asset host lookup failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        }),
        crate::util::ssrf::UrlCheckError::Refused(e) => AssetError::Gone(format!("asset URL refused: {e}")),
    })
}

/// A request that got no answer: asked again next time.
fn unanswered(vendor: &str, e: reqwest::Error) -> AssetError {
    AssetError::Transient(ProviderError::RequestFailed {
        message: format!("{vendor} asset download failed: {e}"),
        status_code: None,
        provider_error: None,
        retryable: true,
    })
}

/// A host's answer that is not the file: gone, or (5xx, 408, 429) a failure
/// that may pass.
fn refused(vendor: &str, status: reqwest::StatusCode) -> AssetError {
    let message = format!("{vendor} asset download returned HTTP {}", status.as_u16());
    if transient(status) {
        AssetError::Transient(ProviderError::RequestFailed {
            message,
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: true,
        })
    } else {
        AssetError::Gone(message)
    }
}

/// A file whose declared size is over `limit`.
fn over_limit(len: u64, limit: u64) -> AssetError {
    AssetError::Gone(format!("asset is {len} bytes, over the {limit}-byte limit"))
}

/// An answer's content type without its parameters, else
/// `application/octet-stream`.
fn content_type_of(headers: &reqwest::header::HeaderMap) -> String {
    headers
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|ct| ct.split(';').next())
        .map(|ct| ct.trim().to_string())
        .filter(|ct| !ct.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string())
}

/// Download one asset into memory, refusing anything over `limit` bytes
/// before buffering it, and — with `public_only` — anything that does not
/// resolve to a public address before connecting: a result naming
/// `http://169.254.169.254/…` would otherwise be fetched from inside the
/// deployment and served back as a mesh. No credentials are sent; result
/// URLs are signed or public, and a key must never reach another host.
/// `vendor` labels the errors ("fal 3d", "tripo").
pub(crate) async fn fetch_asset(
    client: &reqwest::Client,
    url: &str,
    limit: u64,
    public_only: bool,
    vendor: &str,
) -> Result<(Vec<u8>, String), AssetError> {
    check_public(url, public_only, vendor).await?;
    let mut resp = client.get(url).send().await.map_err(|e| unanswered(vendor, e))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(refused(vendor, status));
    }
    if let Some(len) = resp.content_length().filter(|&l| l > limit) {
        return Err(over_limit(len, limit));
    }
    let content_type = content_type_of(resp.headers());
    // Streamed against the limit, so a missing or dishonest Content-Length
    // cannot make us buffer an arbitrarily large body.
    let mut bytes = Vec::with_capacity(resp.content_length().unwrap_or(0) as usize);
    while let Some(chunk) = resp.chunk().await.map_err(|e| {
        AssetError::Transient(ProviderError::RequestFailed {
            message: format!("{vendor} asset body failed: {e}"),
            status_code: Some(status.as_u16()),
            provider_error: None,
            retryable: true,
        })
    })? {
        if (bytes.len() + chunk.len()) as u64 > limit {
            return Err(AssetError::Gone(format!("asset exceeds the {limit}-byte limit")));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, content_type))
}

/// The whole file's size a `Content-Range` names (`bytes 0-0/1234`, or
/// `bytes */0` beside a 416); `None` when it is `*` or the header is missing.
fn range_total(headers: &reqwest::header::HeaderMap) -> Option<u64> {
    headers.get(reqwest::header::CONTENT_RANGE)?.to_str().ok()?.rsplit_once('/')?.1.trim().parse().ok()
}

/// Check one asset's link without downloading it, for a job that keeps
/// nothing ([`Fetch::LinksOnly`](crate::providers::Fetch::LinksOnly)): one
/// GET of its first byte (`Range: bytes=0-0`). Never HEAD: a presigned link
/// refuses it, since its signature covers the method. The answer's body is
/// never read, so a host that ignores the range and sends the whole file is
/// hung up on after the head. [`fetch_asset`]'s rules hold: the
/// public-address check, `limit` against the size the host declares, and the
/// same split between a link that is gone and a failure that may pass.
/// Hands back the file's size, when the host says it, and its content type.
pub(crate) async fn probe_asset(
    client: &reqwest::Client,
    url: &str,
    limit: u64,
    public_only: bool,
    vendor: &str,
) -> Result<(Option<u64>, String), AssetError> {
    check_public(url, public_only, vendor).await?;
    let resp = client.get(url).header(reqwest::header::RANGE, "bytes=0-0").send().await.map_err(|e| unanswered(vendor, e))?;
    let (size, content_type) = match resp.status() {
        reqwest::StatusCode::PARTIAL_CONTENT => (range_total(resp.headers()), content_type_of(resp.headers())),
        // The first byte of an empty file is out of range: the file is there.
        reqwest::StatusCode::RANGE_NOT_SATISFIABLE => (range_total(resp.headers()), "application/octet-stream".to_string()),
        // The host ignored the range: the head names the whole file's size.
        status if status.is_success() => (resp.content_length(), content_type_of(resp.headers())),
        status => return Err(refused(vendor, status)),
    };
    // Unread: dropping the answer hangs up on whatever body follows its head.
    drop(resp);
    if let Some(len) = size.filter(|&l| l > limit) {
        return Err(over_limit(len, limit));
    }
    Ok((size, content_type))
}

/// A result file as an adapter hands it on.
pub(crate) struct Fetched {
    /// The file; empty when only its link was checked.
    pub(crate) bytes: Vec<u8>,
    /// The size the host reported, when only the link was checked.
    pub(crate) size_bytes: Option<u64>,
    pub(crate) content_type: String,
}

/// Download one asset ([`fetch_asset`]), or, for a job that keeps nothing
/// (`links_only`, the gate's [`Fetch::LinksOnly`](crate::providers::Fetch::LinksOnly)),
/// only check its link ([`probe_asset`]). Either fails the same way.
pub(crate) async fn fetch_or_check(
    client: &reqwest::Client,
    url: &str,
    limit: u64,
    public_only: bool,
    vendor: &str,
    links_only: bool,
) -> Result<Fetched, AssetError> {
    if links_only {
        let (size_bytes, content_type) = probe_asset(client, url, limit, public_only, vendor).await?;
        Ok(Fetched { bytes: Vec::new(), size_bytes, content_type })
    } else {
        let (bytes, content_type) = fetch_asset(client, url, limit, public_only, vendor).await?;
        Ok(Fetched { bytes, size_bytes: None, content_type })
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use wiremock::matchers::{header, method, path};
    use wiremock::{Mock, MockServer, ResponseTemplate};

    const MIB: u64 = 1024 * 1024;

    fn client() -> reqwest::Client {
        crate::util::ssrf::public_only_download_client(std::time::Duration::from_secs(600), std::time::Duration::from_secs(60))
    }

    /// A link check's answer, with a failure spelled out for the assertions.
    fn verdict(r: Result<(Option<u64>, String), AssetError>) -> Result<(Option<u64>, String), String> {
        r.map_err(|e| match e {
            AssetError::Gone(why) => format!("gone: {why}"),
            AssetError::Transient(e) => format!("transient: {e}"),
        })
    }

    /// A host that answers every request with `answer`, sent as it is, and
    /// then hangs up. It hands back the head of each request it read.
    async fn raw_host(answer: &'static [u8]) -> (String, tokio::sync::mpsc::UnboundedReceiver<String>) {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let url = format!("http://{}", listener.local_addr().unwrap());
        let (tx, rx) = tokio::sync::mpsc::unbounded_channel();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                let tx = tx.clone();
                tokio::spawn(async move {
                    let mut head = Vec::new();
                    let mut buf = [0u8; 1024];
                    while !head.windows(4).any(|w| w == b"\r\n\r\n") {
                        match sock.read(&mut buf).await {
                            Ok(0) | Err(_) => break,
                            Ok(n) => head.extend_from_slice(&buf[..n]),
                        }
                    }
                    let _ = tx.send(String::from_utf8_lossy(&head).into_owned());
                    let _ = sock.write_all(answer).await;
                    let _ = sock.shutdown().await;
                });
            }
        });
        (url, rx)
    }

    /// A presigned GET link refuses HEAD (its signature covers the method),
    /// so a link is checked with a GET of its first byte, and the file's size
    /// is the total its range names.
    #[tokio::test]
    async fn a_link_check_gets_one_byte_and_reads_the_size_from_content_range() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/cdn/model.glb"))
            .and(header("range", "bytes=0-0"))
            .respond_with(
                ResponseTemplate::new(206)
                    .insert_header("content-range", "bytes 0-0/5000000")
                    .insert_header("content-type", "model/gltf-binary")
                    .set_body_bytes(b"g".to_vec()),
            )
            .mount(&server)
            .await;
        let url = format!("{}/cdn/model.glb", server.uri());
        let found = verdict(probe_asset(&client(), &url, 128 * MIB, false, "test").await);
        assert_eq!(found, Ok((Some(5_000_000), "model/gltf-binary".to_string())));
        let asked = server.received_requests().await.unwrap();
        assert_eq!(asked.iter().map(|r| r.method.as_str()).collect::<Vec<_>>(), ["GET"], "one GET per link");
    }

    /// The first bytes of a 100 MiB body, then the host hangs up: a client
    /// that read on would fail.
    const WHOLE_FILE: &[u8] =
        b"HTTP/1.1 200 OK\r\ncontent-type: model/gltf-binary\r\ncontent-length: 104857600\r\n\r\nglTF\x02\x00\x00\x00 the start of a big file";

    /// A host that ignores the range answers 200 with the whole file: litegen
    /// reads the answer's head, takes the size from its Content-Length, and
    /// drops the body unread.
    #[tokio::test]
    async fn a_link_check_reads_only_the_head_when_the_host_sends_the_whole_file() {
        let (base, mut heads) = raw_host(WHOLE_FILE).await;
        let found = verdict(probe_asset(&client(), &format!("{base}/cdn/model.glb"), 128 * MIB, false, "test").await);
        assert_eq!(found, Ok((Some(100 * MIB), "model/gltf-binary".to_string())), "the size from the head, the body unread");
        let head = heads.recv().await.expect("the request").to_ascii_lowercase();
        assert!(head.starts_with("get /cdn/model.glb "), "{head}");
        assert!(head.contains("\r\nrange: bytes=0-0\r\n"), "{head}");
    }

    /// The download's size limit holds for a link check, by the size the
    /// host declares.
    #[tokio::test]
    async fn a_link_check_refuses_a_file_over_the_limit_by_its_declared_size() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/cdn/huge.glb"))
            .and(header("range", "bytes=0-0"))
            .respond_with(ResponseTemplate::new(206).insert_header("content-range", format!("bytes 0-0/{}", 200 * MIB)).set_body_bytes(b"g".to_vec()))
            .mount(&server)
            .await;
        let found = verdict(probe_asset(&client(), &format!("{}/cdn/huge.glb", server.uri()), 128 * MIB, false, "test").await);
        let why = found.expect_err("a file over the limit");
        assert!(why.starts_with("gone: ") && why.contains("over the"), "{why}");
    }

    /// The first byte of an empty file is out of range (416, `bytes */0`):
    /// the link works, as a download of the empty file would.
    #[tokio::test]
    async fn a_link_check_finds_an_empty_file() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .and(path("/cdn/empty.png"))
            .and(header("range", "bytes=0-0"))
            .respond_with(ResponseTemplate::new(416).insert_header("content-range", "bytes */0"))
            .mount(&server)
            .await;
        let found = verdict(probe_asset(&client(), &format!("{}/cdn/empty.png", server.uri()), MIB, false, "test").await);
        assert_eq!(found, Ok((Some(0), "application/octet-stream".to_string())));
    }

    /// A link check sorts a failure as a download does: a link that is gone
    /// fails the job, and one that may answer next time is asked again.
    #[tokio::test]
    async fn a_link_check_sorts_failures_as_a_download_does() {
        let server = MockServer::start().await;
        let cases = [("/gone", 404, true), ("/expired", 403, true), ("/busy", 503, false), ("/limited", 429, false)];
        for (at, status, _) in cases {
            Mock::given(method("GET")).and(path(at)).respond_with(ResponseTemplate::new(status)).mount(&server).await;
        }
        for (at, _, gone) in cases {
            match probe_asset(&client(), &format!("{}{at}", server.uri()), MIB, false, "test").await {
                Err(AssetError::Gone(why)) => assert!(gone, "{at}: a passing failure was taken as gone: {why}"),
                Err(AssetError::Transient(e)) => assert!(!gone && e.is_retryable(), "{at}: {e}"),
                Ok(found) => panic!("{at}: a failing link passed: {found:?}"),
            }
        }
    }

    /// The public-address check runs before a link check connects.
    #[tokio::test]
    async fn a_link_check_never_reaches_a_private_address() {
        let server = MockServer::start().await;
        Mock::given(method("GET"))
            .respond_with(ResponseTemplate::new(206).insert_header("content-range", "bytes 0-0/10").set_body_bytes(b"g".to_vec()))
            .mount(&server)
            .await;
        let found = verdict(probe_asset(&client(), &format!("{}/cdn/model.glb", server.uri()), MIB, true, "test").await);
        let why = found.expect_err("a private address");
        assert!(why.starts_with("gone: ") && why.contains("refused"), "{why}");
        assert!(server.received_requests().await.unwrap().is_empty(), "nothing was sent to a private address");
    }
}
