//! SSRF guard for server-side outbound fetches.
//!
//! Used by features that fetch a *user-supplied* URL from the server
//! (reference-image materialization, webhook dispatch). It rejects URLs whose
//! scheme is not http(s) or whose host resolves to a non-public address
//! (loopback, private, link-local — including the `169.254.169.254` cloud
//! metadata endpoint —, unique-local, CGNAT, etc.).
//!
//! The HTTP client looks the host up again when it connects, after this
//! check, so a name whose answer changes in between (DNS rebinding) could
//! reach an internal address. The `public_only_*` clients close that window:
//! their [`PublicOnlyResolver`] judges every address they connect to, on
//! every attempt. Use them for any URL a caller names. Callers MUST also
//! disable redirect following (every client here does), so a public host
//! cannot 3xx the request into a private target.

use std::net::{IpAddr, Ipv4Addr};

/// True if `ip` must never be the target of a server-side fetch.
pub fn is_disallowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            let o = v4.octets();
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local() // includes 169.254.169.254 metadata
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.is_multicast()
                || o[0] == 0 // 0.0.0.0/8 "this host"
                // 100.64.0.0/10 carrier-grade NAT
                || (o[0] == 100 && (o[1] & 0xc0) == 0x40)
                // 240.0.0.0/4 reserved (Class E; pod ranges on some clouds)
                || o[0] >= 240
                // 198.18.0.0/15 benchmarking
                || (o[0] == 198 && (o[1] & 0xfe) == 18)
                // 192.0.0.0/24 IETF protocol assignments (192.0.0.192 is a
                // metadata address on some clouds)
                || (o[0] == 192 && o[1] == 0 && o[2] == 0)
                // 192.88.99.0/24 6to4 relay anycast
                || (o[0] == 192 && o[1] == 88 && o[2] == 99)
                // Azure's WireServer: a public address that answers only
                // from inside the VM.
                || o == [168, 63, 129, 16]
        }
        IpAddr::V6(v6) => {
            // Treat IPv4-mapped/compatible addresses as their embedded v4.
            if let Some(v4) = v6.to_ipv4_mapped() {
                return is_disallowed_ip(IpAddr::V4(v4));
            }
            if let Some(v4) = v6.to_ipv4() {
                return is_disallowed_ip(IpAddr::V4(v4));
            }
            // NAT64 well-known prefix 64:ff9b::/96 (RFC 6052) embeds an IPv4
            // address in the low 32 bits. On a DNS64/NAT64 network this is how an
            // internal v4 target is reached over v6, so e.g. `64:ff9b::10.0.0.1`
            // must be evaluated as 10.0.0.1. `to_ipv4`/`to_ipv4_mapped` do NOT
            // cover this prefix, so decode it explicitly.
            let o = v6.octets();
            if o[0] == 0x00
                && o[1] == 0x64
                && o[2] == 0xff
                && o[3] == 0x9b
                && o[4..12].iter().all(|&b| b == 0)
            {
                let v4 = Ipv4Addr::new(o[12], o[13], o[14], o[15]);
                return is_disallowed_ip(IpAddr::V4(v4));
            }
            // 6to4 2002::/16 embeds an IPv4 address in bits 16–48, and SIIT
            // ::ffff:0:0/96 in the low 32 bits: judge them as that address.
            if o[0] == 0x20 && o[1] == 0x02 {
                return is_disallowed_ip(IpAddr::V4(Ipv4Addr::new(o[2], o[3], o[4], o[5])));
            }
            if o[..8].iter().all(|&b| b == 0) && o[8] == 0xff && o[9] == 0xff && o[10] == 0 && o[11] == 0 {
                return is_disallowed_ip(IpAddr::V4(Ipv4Addr::new(o[12], o[13], o[14], o[15])));
            }
            v6.is_loopback()
                || v6.is_unspecified()
                || v6.is_multicast()
                // fc00::/7 unique local
                || (o[0] & 0xfe) == 0xfc
                // fe80::/10 link local
                || (o[0] == 0xfe && (o[1] & 0xc0) == 0x80)
                // fec0::/10 site local (deprecated, still routed by some)
                || (o[0] == 0xfe && (o[1] & 0xc0) == 0xc0)
                // 64:ff9b:1::/48 local-use NAT64: which IPv4 it reaches is
                // the network's choice.
                || o[..6] == [0x00, 0x64, 0xff, 0x9b, 0x00, 0x01]
                // 2001::/32 Teredo: the IPv4 address in it is obfuscated.
                || o[..4] == [0x20, 0x01, 0x00, 0x00]
                // 2001:db8::/32 documentation
                || o[..4] == [0x20, 0x01, 0x0d, 0xb8]
                // 100::/64 discard-only
                || (o[..2] == [0x01, 0x00] && o[2..8].iter().all(|&b| b == 0))
        }
    }
}

/// Validate that `url` is safe to fetch server-side. Returns `Err(reason)` if
/// the scheme is unsupported, the host is missing, fails to resolve, or
/// resolves to any disallowed address.
/// Why [`check_public_url`] refused a URL.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UrlCheckError {
    /// The resolver failed (timeout, SERVFAIL, EAI_AGAIN, no answer). Asking
    /// again later can succeed, so a caller fetching a vendor's result should
    /// retry rather than fail a finished job over one resolver blip.
    Lookup(String),
    /// The URL itself is unacceptable: bad scheme, no host, or an address
    /// inside the deployment. It never will be.
    Refused(String),
}

impl std::fmt::Display for UrlCheckError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            UrlCheckError::Lookup(s) | UrlCheckError::Refused(s) => f.write_str(s),
        }
    }
}

/// [`check_public_url`] with the reason flattened to text, for callers that
/// treat every refusal alike.
pub async fn validate_public_url(url: &str) -> Result<(), String> {
    check_public_url(url).await.map_err(|e| e.to_string())
}

pub async fn check_public_url(url: &str) -> Result<(), UrlCheckError> {
    public_addrs(url).await.map(|_| ())
}

/// A client for fetching `url` that cannot be steered inside the deployment:
/// it follows no redirects, and its host is pinned to the public addresses
/// [`check_public_url`]'s check approved, which closes the DNS-rebinding
/// window described above. For URLs anyone may make LiteGen fetch, such as
/// an OAuth client's metadata document.
pub async fn pinned_public_client(url: &str, timeout: std::time::Duration) -> Result<reqwest::Client, UrlCheckError> {
    let (host, addrs) = public_addrs(url).await?;
    // The pinned addresses answer for `host`; any other name goes through
    // the guard as well, and no proxy resolves anything for it.
    let builder = public_only_builder(timeout);
    let builder = if addrs.is_empty() { builder } else { builder.resolve_to_addrs(&host, &addrs) };
    builder.build().map_err(|e| UrlCheckError::Refused(format!("http client: {e}")))
}

/// The URL's host and the addresses it resolves to, all public (none for an
/// IP literal, which is checked as written), or why it may not be fetched.
async fn public_addrs(url: &str) -> Result<(String, Vec<std::net::SocketAddr>), UrlCheckError> {
    let parsed = reqwest::Url::parse(url).map_err(|e| UrlCheckError::Refused(format!("invalid url: {e}")))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(UrlCheckError::Refused(format!("unsupported url scheme: {other}"))),
    }

    let host = parsed.host_str().ok_or_else(|| UrlCheckError::Refused("url has no host".to_string()))?;
    let port = parsed.port_or_known_default().unwrap_or(443);

    // If the host is an IP literal, check it directly (no DNS). `host_str()`
    // keeps brackets for IPv6 literals (e.g. "[::1]"), which `lookup_host`
    // rejects — so strip them and parse here, ensuring `[::1]`/`[fc00::1]` are
    // evaluated by `is_disallowed_ip` rather than merely failing to resolve.
    let host_unbracketed = host
        .strip_prefix('[')
        .and_then(|h| h.strip_suffix(']'))
        .unwrap_or(host);
    if let Ok(ip) = host_unbracketed.parse::<IpAddr>() {
        if is_disallowed_ip(ip) {
            return Err(UrlCheckError::Refused("url targets a disallowed (private/loopback/link-local) address".to_string()));
        }
        return Ok((host.to_string(), Vec::new()));
    }

    // Otherwise it's a hostname: resolve it and check every address it maps to.
    let addrs: Vec<std::net::SocketAddr> = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| UrlCheckError::Lookup(format!("dns resolution failed: {e}")))?
        .collect();
    if addrs.iter().any(|addr| is_disallowed_ip(addr.ip())) {
        return Err(UrlCheckError::Refused("url resolves to a disallowed (private/loopback/link-local) address".to_string()));
    }
    if addrs.is_empty() {
        return Err(UrlCheckError::Lookup("host did not resolve".to_string()));
    }
    Ok((host.to_string(), addrs))
}

/// Total-request timeout for server-side fetches of user-supplied URLs. A
/// hostile or hung host must never be able to pin a webhook/reference-fetch task
/// (and its connection) open forever.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Connect-phase timeout (a subset of the total), so a host that accepts the TCP
/// SYN but never completes the handshake is bounded too.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

/// A client that follows no redirects, so a host can't 3xx a request
/// somewhere else, with a bounded time. It connects wherever its URL's name
/// resolves, so use it only for hosts the operator chose: a single-tenant
/// webhook, a vendor's own API. For any URL a caller names, use the
/// `public_only_*` clients.
pub fn no_redirect_client() -> reqwest::Client {
    no_redirect_client_with_timeout(FETCH_TIMEOUT)
}

/// Same as [`no_redirect_client`] but with a caller-chosen total timeout. Kept
/// separate so tests can exercise the timeout behaviour with a short deadline.
pub fn no_redirect_client_with_timeout(timeout: std::time::Duration) -> reqwest::Client {
    no_redirect_builder(timeout).build().unwrap_or_else(|_| reqwest::Client::new())
}

/// Resolves names the way the system does, then drops every address litegen
/// must not connect to ([`is_disallowed_ip`]). A name left with none fails
/// to connect.
///
/// [`check_public_url`] looks a host up and checks it, then the HTTP client
/// looks it up again when it connects, possibly more than once (webhook
/// retries). A name whose answer moves inside the deployment in between (DNS
/// rebinding) would otherwise reach it. With this resolver every connection
/// is judged by the address it actually goes to.
///
/// An IP literal never reaches a resolver (the connector parses it itself),
/// so [`check_public_url`]'s literal check still has to run first. The
/// guarded clients connect directly, never through an `HTTP(S)_PROXY`: a
/// proxy would resolve the target itself, out of this resolver's sight.
#[derive(Debug, Default, Clone, Copy)]
pub struct PublicOnlyResolver;

impl reqwest::dns::Resolve for PublicOnlyResolver {
    fn resolve(&self, name: reqwest::dns::Name) -> reqwest::dns::Resolving {
        Box::pin(async move {
            let host = name.as_str().to_string();
            // Port 0: the connector puts the URL's port on each address.
            let found = tokio::net::lookup_host((host.as_str(), 0)).await?;
            let public = public_answers(found);
            if public.is_empty() {
                return Err(format!("{host} resolves to no public address").into());
            }
            Ok(Box::new(public.into_iter()) as reqwest::dns::Addrs)
        })
    }
}

/// The addresses of a lookup litegen may connect to.
fn public_answers(addrs: impl IntoIterator<Item = std::net::SocketAddr>) -> Vec<std::net::SocketAddr> {
    addrs.into_iter().filter(|a| !is_disallowed_ip(a.ip())).collect()
}

/// A builder for every client below: no redirects, bounded time.
fn no_redirect_builder(timeout: std::time::Duration) -> reqwest::ClientBuilder {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(CONNECT_TIMEOUT)
}

/// [`no_redirect_builder`] for a URL a caller or vendor names: no proxy, and
/// [`PublicOnlyResolver`]. With a proxy from the environment, the proxy would
/// pick the address, which defeats the guard: litegen reaches such a URL
/// directly or not at all. Every guarded client, `pinned_public_client`
/// included, is built from this.
fn public_only_builder(timeout: std::time::Duration) -> reqwest::ClientBuilder {
    no_redirect_builder(timeout).no_proxy().dns_resolver(std::sync::Arc::new(PublicOnlyResolver))
}

/// `builder` (from [`public_only_builder`]), built. A builder fails only when
/// no TLS backend can start; `reqwest::Client::new()` panics then too, and an
/// unguarded fallback would quietly drop the guard, so this fails loudly.
fn build_public_only(builder: reqwest::ClientBuilder) -> reqwest::Client {
    builder.build().expect("an HTTP client with the default TLS backend")
}

/// [`no_redirect_client`] that connects only to public addresses
/// ([`PublicOnlyResolver`]). Use it for every URL a caller may name and
/// litegen has checked with [`check_public_url`].
pub fn public_only_client() -> reqwest::Client {
    public_only_client_with_timeout(FETCH_TIMEOUT)
}

/// [`public_only_client`] with a caller-chosen total timeout. The process
/// shares one per timeout and runtime ([`super::http_clients`], P8), as it
/// does the two download clients below.
pub fn public_only_client_with_timeout(timeout: std::time::Duration) -> reqwest::Client {
    super::http_clients::shared(super::http_clients::ClientKey::PublicOnly { timeout }, || {
        build_public_only(public_only_builder(timeout))
    })
}

/// [`no_redirect_download_client`] that connects only to public addresses.
pub fn public_only_download_client(timeout: std::time::Duration, idle: std::time::Duration) -> reqwest::Client {
    super::http_clients::shared(super::http_clients::ClientKey::PublicOnlyDownload { timeout, idle }, || {
        build_public_only(public_only_builder(timeout).read_timeout(idle))
    })
}

/// [`no_redirect_client_with_timeout`] for large downloads: the total `timeout`
/// still caps the transfer, and `idle` additionally fails one that stops
/// sending, so a stalled body costs `idle`, not the whole `timeout`.
pub fn no_redirect_download_client(timeout: std::time::Duration, idle: std::time::Duration) -> reqwest::Client {
    super::http_clients::shared(super::http_clients::ClientKey::NoRedirectDownload { timeout, idle }, || {
        no_redirect_builder(timeout).read_timeout(idle).build().unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// [`no_redirect_upload_client`] that connects only to public addresses.
pub fn public_only_upload_client(timeout: std::time::Duration) -> reqwest::Client {
    super::http_clients::shared(super::http_clients::ClientKey::PublicOnlyUpload { timeout }, || {
        build_public_only(public_only_builder(timeout))
    })
}

/// [`no_redirect_client_with_timeout`] for large uploads: the total
/// `timeout` caps the transfer, and there is no read timeout. reqwest's read
/// timeout runs from sending a request until its answer's head arrives, and
/// nothing resets it while the body is written, so on an upload (a bucket
/// answers a PUT only after its last byte) it would cap the whole transfer.
/// The caller cuts off an upload that stops moving itself. Shared like the
/// download clients, under a key of its own.
pub fn no_redirect_upload_client(timeout: std::time::Duration) -> reqwest::Client {
    super::http_clients::shared(super::http_clients::ClientKey::NoRedirectUpload { timeout }, || {
        no_redirect_builder(timeout).build().unwrap_or_else(|_| reqwest::Client::new())
    })
}

/// Why [`download_capped`] returned no body.
#[derive(Debug)]
pub enum CappedDownloadError {
    /// The request could not be sent, or its body broke off.
    Transport(String),
    /// The host answered with this non-success status.
    Status(reqwest::StatusCode),
    /// The body is over `max` bytes: `bytes` is its declared length, or what
    /// had arrived when litegen stopped reading.
    TooLarge { bytes: u64, max: u64 },
}

/// Download `url` with `client` (a `public_only_*` client, after
/// [`check_public_url`] passed, for any URL a caller or vendor names) into
/// memory, with its `Content-Type`, never
/// holding more than `max` bytes: a declared length over it is refused before
/// reading, and the body is streamed against it, so a missing or dishonest
/// `Content-Length` cannot make litegen buffer whatever a host sends.
pub async fn download_capped(client: &reqwest::Client, url: &str, max: u64) -> Result<(Vec<u8>, Option<String>), CappedDownloadError> {
    let mut resp = client.get(url).send().await.map_err(|e| CappedDownloadError::Transport(e.without_url().to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(CappedDownloadError::Status(status));
    }
    if let Some(n) = resp.content_length().filter(|n| *n > max) {
        return Err(CappedDownloadError::TooLarge { bytes: n, max });
    }
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let mut bytes = Vec::with_capacity(resp.content_length().unwrap_or(0) as usize);
    while let Some(chunk) = resp.chunk().await.map_err(|e| CappedDownloadError::Transport(e.without_url().to_string()))? {
        let seen = (bytes.len() + chunk.len()) as u64;
        if seen > max {
            return Err(CappedDownloadError::TooLarge { bytes: seen, max });
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok((bytes, content_type))
}

/// What [`download_to_file`] wrote.
#[derive(Debug)]
pub struct Spooled {
    /// Bytes written.
    pub len: u64,
    /// Lowercase hex SHA-256 of them.
    pub sha256: String,
    /// The host's `Content-Type`, if it sent one.
    pub content_type: Option<String>,
}

/// [`download_capped`], into `file` instead of memory: each chunk is hashed
/// and written as it arrives, so the download holds one chunk at a time,
/// under the same cap. A failure to write the file (a full disk, a spool
/// directory that went away) is a [`CappedDownloadError::Transport`]: a
/// passing failure, never a verdict on the download.
pub async fn download_to_file(client: &reqwest::Client, url: &str, max: u64, file: &mut tokio::fs::File) -> Result<Spooled, CappedDownloadError> {
    use sha2::Digest;
    use tokio::io::AsyncWriteExt;
    let mut resp = client.get(url).send().await.map_err(|e| CappedDownloadError::Transport(e.without_url().to_string()))?;
    let status = resp.status();
    if !status.is_success() {
        return Err(CappedDownloadError::Status(status));
    }
    if let Some(n) = resp.content_length().filter(|n| *n > max) {
        return Err(CappedDownloadError::TooLarge { bytes: n, max });
    }
    let content_type = resp.headers().get(reqwest::header::CONTENT_TYPE).and_then(|v| v.to_str().ok()).map(str::to_string);
    let written = |e: std::io::Error| CappedDownloadError::Transport(format!("could not write the download to disk: {e}"));
    let (mut hasher, mut len) = (sha2::Sha256::new(), 0u64);
    while let Some(chunk) = resp.chunk().await.map_err(|e| CappedDownloadError::Transport(e.without_url().to_string()))? {
        len += chunk.len() as u64;
        if len > max {
            return Err(CappedDownloadError::TooLarge { bytes: len, max });
        }
        hasher.update(&chunk);
        file.write_all(&chunk).await.map_err(written)?;
    }
    file.flush().await.map_err(written)?;
    Ok(Spooled { len, sha256: hex::encode(hasher.finalize()), content_type })
}

/// (Test-only: shared with proxy/webhook.rs's tests since webhook_client moved there.)
#[cfg(test)]
/// Whether `err`, or anything it wraps, is [`PublicOnlyResolver`] refusing
/// a name with no public address.
pub(crate) fn refused_by_the_guard(err: &reqwest::Error) -> bool {
    let mut cause: Option<&dyn std::error::Error> = Some(err);
    while let Some(e) = cause {
        if e.to_string().contains("resolves to no public address") {
            return true;
        }
        cause = e.source();
    }
    false
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

    /// A host that answers with a chunked body (no `Content-Length`), sends
    /// `total` bytes of it and then stalls without ending it.
    async fn stalling_chunked_host(total: usize) -> String {
        use tokio::io::{AsyncReadExt, AsyncWriteExt};
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let addr = listener.local_addr().unwrap();
        tokio::spawn(async move {
            while let Ok((mut sock, _)) = listener.accept().await {
                tokio::spawn(async move {
                    let mut req = [0u8; 4096];
                    let _ = sock.read(&mut req).await;
                    let _ = sock.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp4\r\nTransfer-Encoding: chunked\r\n\r\n").await;
                    let chunk = vec![7u8; 64 * 1024];
                    for _ in 0..total / chunk.len() {
                        if sock.write_all(format!("{:x}\r\n", chunk.len()).as_bytes()).await.is_err()
                            || sock.write_all(&chunk).await.is_err()
                            || sock.write_all(b"\r\n").await.is_err()
                        {
                            return;
                        }
                    }
                    tokio::time::sleep(std::time::Duration::from_secs(120)).await;
                });
            }
        });
        format!("http://{addr}/clip.mp4")
    }

    #[tokio::test]
    async fn a_body_past_the_cap_is_refused_while_it_streams() {
        let url = stalling_chunked_host(4 * 1024 * 1024).await;
        let client = no_redirect_download_client(std::time::Duration::from_secs(60), std::time::Duration::from_secs(30));
        let got = tokio::time::timeout(std::time::Duration::from_secs(10), download_capped(&client, &url, 1024 * 1024))
            .await
            .expect("refused once the cap is passed, not after waiting for the whole body");
        assert!(matches!(got, Err(CappedDownloadError::TooLarge { max: 1048576, .. })), "{got:?}");
    }

    /// Review 2026-11-11 P4: a download to a file is hashed and capped as it
    /// streams, and what it wrote is what the host sent.
    #[tokio::test]
    async fn a_download_to_a_file_is_hashed_and_capped_as_it_streams() {
        use sha2::Digest;
        let dir = tempfile::tempdir().unwrap();
        let client = no_redirect_download_client(std::time::Duration::from_secs(60), std::time::Duration::from_secs(30));
        let url = stalling_chunked_host(4 * 1024 * 1024).await;
        let mut file = tokio::fs::File::create(dir.path().join("big.part")).await.unwrap();
        let got = tokio::time::timeout(std::time::Duration::from_secs(10), download_to_file(&client, &url, 1024 * 1024, &mut file))
            .await
            .expect("refused once the cap is passed, not after waiting for the whole body");
        assert!(matches!(got, Err(CappedDownloadError::TooLarge { max: 1048576, .. })), "{got:?}");

        let server = wiremock::MockServer::start().await;
        wiremock::Mock::given(wiremock::matchers::method("GET"))
            .respond_with(wiremock::ResponseTemplate::new(200).set_body_raw(b"a small clip".to_vec(), "video/webm"))
            .mount(&server)
            .await;
        let path = dir.path().join("small.part");
        let mut file = tokio::fs::File::create(&path).await.unwrap();
        let got = download_to_file(&client, &server.uri(), 1024, &mut file).await.unwrap();
        assert_eq!((got.len, got.content_type.as_deref()), (12, Some("video/webm")));
        assert_eq!(got.sha256, hex::encode(sha2::Sha256::digest(b"a small clip")));
        assert_eq!(std::fs::read(&path).unwrap(), b"a small clip");
    }

    #[tokio::test]
    async fn a_download_reports_its_status_or_returns_the_body_and_its_type() {
        use wiremock::matchers::{method, path};
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET")).and(path("/gone")).respond_with(ResponseTemplate::new(404)).mount(&server).await;
        Mock::given(method("GET")).and(path("/moved"))
            .respond_with(ResponseTemplate::new(302).insert_header("location", "http://169.254.169.254/latest"))
            .mount(&server).await;
        Mock::given(method("GET")).and(path("/ok"))
            .respond_with(ResponseTemplate::new(200).set_body_raw(vec![1u8; 10], "image/png"))
            .mount(&server).await;
        let client = no_redirect_download_client(std::time::Duration::from_secs(10), std::time::Duration::from_secs(5));
        let got = download_capped(&client, &format!("{}/gone", server.uri()), 100).await;
        assert!(matches!(got, Err(CappedDownloadError::Status(s)) if s == 404), "{got:?}");
        let got = download_capped(&client, &format!("{}/moved", server.uri()), 100).await;
        assert!(matches!(got, Err(CappedDownloadError::Status(s)) if s == 302), "a redirect is not followed: {got:?}");
        let got = download_capped(&client, &format!("{}/ok", server.uri()), 9).await;
        assert!(matches!(got, Err(CappedDownloadError::TooLarge { bytes: 10, max: 9 })), "{got:?}");
        let (bytes, ct) = download_capped(&client, &format!("{}/ok", server.uri()), 10).await.unwrap();
        assert_eq!((bytes, ct.as_deref()), (vec![1u8; 10], Some("image/png")));
    }

    #[tokio::test]
    async fn a_lookup_failure_is_told_apart_from_a_refusal() {
        assert!(matches!(
            check_public_url("https://litegen-ssrf-test.invalid/x").await,
            Err(UrlCheckError::Lookup(_))
        ));
        assert!(matches!(check_public_url("http://127.0.0.1/x").await, Err(UrlCheckError::Refused(_))));
        assert!(matches!(check_public_url("ftp://example.com/x").await, Err(UrlCheckError::Refused(_))));
        // The string form callers already use is unchanged.
        assert!(validate_public_url("http://127.0.0.1/x").await.unwrap_err().contains("disallowed"));
    }

    #[tokio::test]
    async fn no_redirect_client_times_out_on_a_hung_host() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};

        let server = MockServer::start().await;
        // Server accepts the connection but stalls for 4s before responding.
        Mock::given(method("GET"))
            .respond_with(
                ResponseTemplate::new(200).set_delay(std::time::Duration::from_secs(4)),
            )
            .mount(&server)
            .await;

        // A 1s timeout must abort the fetch well before the 4s response — a
        // client with no timeout would instead block until the response lands.
        let client = no_redirect_client_with_timeout(std::time::Duration::from_secs(1));
        let start = std::time::Instant::now();
        let result = client.get(server.uri()).send().await;
        let elapsed = start.elapsed();

        assert!(result.is_err(), "request to a hung host must time out, not hang");
        assert!(
            elapsed < std::time::Duration::from_secs(3),
            "timeout must fire near its 1s deadline, took {elapsed:?}"
        );
    }

    #[test]
    fn blocks_private_and_metadata_v4() {
        for ip in [
            "169.254.169.254", // cloud metadata (link-local)
            "127.0.0.1",       // loopback
            "10.0.0.5",        // private
            "192.168.1.1",     // private
            "172.16.0.1",      // private
            "0.0.0.0",         // unspecified
            "100.64.1.1",      // CGNAT
        ] {
            let a: Ipv4Addr = ip.parse().unwrap();
            assert!(is_disallowed_ip(IpAddr::V4(a)), "{ip} should be blocked");
        }
    }

    #[test]
    fn allows_public_v4() {
        for ip in ["8.8.8.8", "1.1.1.1", "93.184.216.34"] {
            let a: Ipv4Addr = ip.parse().unwrap();
            assert!(!is_disallowed_ip(IpAddr::V4(a)), "{ip} should be allowed");
        }
    }

    #[test]
    fn blocks_v6_loopback_and_mapped_private() {
        assert!(is_disallowed_ip(IpAddr::V6(Ipv6Addr::LOCALHOST)));
        // ::ffff:10.0.0.1 (IPv4-mapped private)
        let mapped: Ipv6Addr = "::ffff:10.0.0.1".parse().unwrap();
        assert!(is_disallowed_ip(IpAddr::V6(mapped)));
        // fe80:: link local
        let ll: Ipv6Addr = "fe80::1".parse().unwrap();
        assert!(is_disallowed_ip(IpAddr::V6(ll)));
        // fc00:: unique local
        let ula: Ipv6Addr = "fc00::1".parse().unwrap();
        assert!(is_disallowed_ip(IpAddr::V6(ula)));
    }

    /// Review of security fix 2: the other special-purpose ranges a fetch
    /// must not reach. Some are internal on real clouds: Class E pod ranges
    /// on GKE, OCI Classic metadata at 192.0.0.192, Azure's WireServer.
    /// Others embed an IPv4 address: 6to4 and SIIT.
    #[test]
    fn blocks_the_other_special_purpose_ranges() {
        for ip in [
            "240.0.0.1", "255.255.255.254",   // reserved (Class E)
            "198.18.0.1", "198.19.255.254",   // benchmarking
            "192.0.0.192", "192.0.0.1",       // IETF protocol assignments
            "192.88.99.1",                    // 6to4 relay anycast
            "224.0.0.1", "239.255.255.250",   // multicast
            "168.63.129.16",                  // Azure WireServer
            "fec0::1",                        // site-local
            "64:ff9b:1::a00:1",               // local-use NAT64
            "2002:7f00:1::1", "2002:a00:1::1", // 6to4 of 127.0.0.1 and 10.0.0.1
            "2001:0:4136:e378:8000:63bf:3fff:fdd2", // Teredo
            "::ffff:0:7f00:1",                // SIIT of 127.0.0.1
            "ff02::1",                        // multicast
            "2001:db8::1",                    // documentation
            "100::1",                         // discard-only
        ] {
            assert!(is_disallowed_ip(ip.parse().unwrap()), "{ip} should be blocked");
        }
        for ip in [
            "93.184.216.34", "198.20.0.1", "192.0.1.1", "223.255.255.255", "168.63.129.17",
            "2606:2800:220:1:248:1893:25c8:1946",
            "2002:5db8:d822::1", // 6to4 of the public 93.184.216.34
        ] {
            assert!(!is_disallowed_ip(ip.parse().unwrap()), "{ip} should be allowed");
        }
    }

    #[test]
    fn blocks_nat64_embedded_private_v4() {
        // 64:ff9b::10.0.0.1 — NAT64 well-known prefix wrapping a private v4.
        let nat64_private: Ipv6Addr = "64:ff9b::a00:1".parse().unwrap();
        assert!(is_disallowed_ip(IpAddr::V6(nat64_private)));
        // 64:ff9b::169.254.169.254 — wrapping the cloud metadata endpoint.
        let nat64_meta: Ipv6Addr = "64:ff9b::a9fe:a9fe".parse().unwrap();
        assert!(is_disallowed_ip(IpAddr::V6(nat64_meta)));
        // 64:ff9b::8.8.8.8 — wrapping a public v4 stays allowed.
        let nat64_public: Ipv6Addr = "64:ff9b::808:808".parse().unwrap();
        assert!(!is_disallowed_ip(IpAddr::V6(nat64_public)));
    }

    #[tokio::test]
    async fn rejects_bad_scheme_and_literal_private() {
        assert!(validate_public_url("file:///etc/passwd").await.is_err());
        assert!(validate_public_url("ftp://example.com/x").await.is_err());
        assert!(validate_public_url("http://169.254.169.254/latest/meta-data/").await.is_err());
        assert!(validate_public_url("http://127.0.0.1:6379/").await.is_err());
        assert!(validate_public_url("http://10.0.0.1/").await.is_err());
        // Bracketed IPv6 literals are evaluated directly (not via DNS failure).
        assert!(validate_public_url("http://[::1]/").await.is_err());
        assert!(validate_public_url("http://[fc00::1]/").await.is_err());
        assert!(validate_public_url("http://[fe80::1]/").await.is_err());
    }

    /// Security fix 2 (2026-11-05): a URL is checked by DNS, then the client
    /// looks the name up again when it connects. A guarded client judges
    /// every address it connects to, so a name whose answer moves inside the
    /// deployment after the check (DNS rebinding) reaches nothing.
    /// `localhost` stands in for such a name: it resolves to loopback.
    #[tokio::test]
    async fn a_public_only_client_cannot_reach_a_name_inside_the_deployment() {
        use wiremock::matchers::method;
        use wiremock::{Mock, MockServer, ResponseTemplate};
        let server = MockServer::start().await;
        Mock::given(method("GET")).respond_with(ResponseTemplate::new(200)).mount(&server).await;
        let by_name = format!("http://localhost:{}/x", server.address().port());

        // Reachable at all: the plain no-redirect client gets there.
        let plain = no_redirect_client_with_timeout(std::time::Duration::from_secs(5));
        assert_eq!(plain.get(&by_name).send().await.expect("the server is up").status(), 200);

        let short = std::time::Duration::from_secs(5);
        for (name, client) in [
            ("public_only_client_with_timeout", public_only_client_with_timeout(short)),
            ("public_only_download_client", public_only_download_client(short, short)),
            ("public_only_client", public_only_client()),
        ] {
            let err = client.get(&by_name).send().await.expect_err("must not connect to loopback");
            assert!(refused_by_the_guard(&err), "{name}: refused by the guard, not by chance: {err:?}");
        }
        assert_eq!(server.received_requests().await.unwrap().len(), 1, "only the plain client's request arrived");
    }

    /// P8 (code review 2026-11-11): the download clients are shared, and a
    /// shared client keeps the settings of the builder it stands for. Two
    /// `no_redirect_download_client`s reach a stand-in over one connection,
    /// and still follow no redirect. The public-only client with the very
    /// same timeouts is another client: a key without its builder would hand
    /// one's settings to the other, the guard included.
    #[tokio::test]
    async fn download_clients_are_shared_and_keep_their_settings() {
        use super::super::http_clients::{builds, ClientKey};
        use std::sync::atomic::Ordering::SeqCst;
        let app = axum::Router::new()
            .route("/file", axum::routing::get(|| async { "bytes" }))
            .route("/moved", axum::routing::get(|| async { (axum::http::StatusCode::FOUND, [("location", "/file")]) }));
        let (base, accepted) = super::super::counting_vendor(app).await;
        let (timeout, idle) = (std::time::Duration::from_secs(41), std::time::Duration::from_secs(7));
        for _ in 0..2 {
            let r = no_redirect_download_client(timeout, idle).get(format!("{base}/file")).send().await.unwrap();
            assert_eq!(r.text().await.unwrap(), "bytes");
            tokio::time::sleep(std::time::Duration::from_millis(50)).await;
        }
        assert_eq!(accepted.load(SeqCst), 1, "two downloads opened one connection");
        let moved = no_redirect_download_client(timeout, idle).get(format!("{base}/moved")).send().await.unwrap();
        assert_eq!(moved.status(), 302, "the shared client still follows no redirect");
        let by_name = base.replace("127.0.0.1", "localhost");
        let err = public_only_download_client(timeout, idle).get(format!("{by_name}/file")).send().await.expect_err("must not connect to loopback");
        assert!(refused_by_the_guard(&err), "the public-only client kept its guard: {err:?}");
        let _ = public_only_download_client(timeout, idle);
        assert_eq!(builds(&ClientKey::NoRedirectDownload { timeout, idle }), 1);
        assert_eq!(builds(&ClientKey::PublicOnlyDownload { timeout, idle }), 1);
    }

    /// Code review 2026-11-11, payloads M1: an upload client has no read
    /// timeout. reqwest's read timeout runs from sending a request until its
    /// answer's head arrives, and nothing resets it while the body is
    /// written, so on a PUT it caps the whole upload (a bucket answers only
    /// after the last byte). The upload clients are shared like the download
    /// clients, and a shared one keeps its own settings: download clients
    /// with the same total time and a short read timeout, built first, lend
    /// the uploads none of it, and the redirect rule and the guard stay.
    #[tokio::test]
    async fn upload_clients_are_shared_and_keep_no_read_timeout() {
        use super::super::http_clients::{builds, ClientKey};
        let (timeout, idle) = (std::time::Duration::from_secs(43), std::time::Duration::from_millis(200));
        let app = axum::Router::new()
            .route("/slow", axum::routing::put(|| async {
                tokio::time::sleep(std::time::Duration::from_millis(700)).await;
                "stored"
            }))
            .route("/moved", axum::routing::put(|| async { (axum::http::StatusCode::FOUND, [("location", "/slow")]) }));
        let (base, _) = super::super::counting_vendor(app).await;
        let download = no_redirect_download_client(timeout, idle);
        let _ = public_only_download_client(timeout, idle);
        assert!(download.put(format!("{base}/slow")).body("x").send().await.is_err(), "precondition: a 200 ms read timeout cuts off an answer that takes 700 ms");
        assert!(format!("{download:?}").contains("read_timeout"), "precondition: a client's Debug names its read timeout: {download:?}");
        for (name, client, again) in [
            ("no_redirect_upload_client", no_redirect_upload_client(timeout), no_redirect_upload_client(timeout)),
            ("public_only_upload_client", public_only_upload_client(timeout), public_only_upload_client(timeout)),
        ] {
            assert!(!format!("{client:?}").contains("read_timeout"), "{name} has no read timeout: {client:?}");
            let stored = client.put(format!("{base}/slow")).body("x").send().await.unwrap_or_else(|e| panic!("{name} waits for a slow answer: {e:?}"));
            assert_eq!(stored.text().await.unwrap(), "stored", "{name}");
            let moved = again.put(format!("{base}/moved")).body("x").send().await.unwrap();
            assert_eq!(moved.status(), 302, "{name} follows no redirect");
        }
        let by_name = base.replace("127.0.0.1", "localhost");
        let err = public_only_upload_client(timeout).put(format!("{by_name}/slow")).body("x").send().await.expect_err("must not connect to loopback");
        assert!(refused_by_the_guard(&err), "the public-only upload client kept its guard: {err:?}");
        assert_eq!(builds(&ClientKey::NoRedirectUpload { timeout }), 1);
        assert_eq!(builds(&ClientKey::PublicOnlyUpload { timeout }), 1);
    }

    #[test]
    fn public_answers_drop_every_internal_address() {
        let addrs: Vec<std::net::SocketAddr> = [
            "127.0.0.1:0", "93.184.216.34:0", "10.0.0.1:0", "169.254.169.254:0", "[::1]:0",
            "[2606:2800:220:1:248:1893:25c8:1946]:0", "[::ffff:192.168.0.1]:0", "[fd00::1]:0",
        ]
        .iter()
        .map(|a| a.parse().unwrap())
        .collect();
        let kept: Vec<String> = public_answers(addrs).iter().map(|a| a.ip().to_string()).collect();
        assert_eq!(kept, ["93.184.216.34", "2606:2800:220:1:248:1893:25c8:1946"]);
    }

    #[tokio::test]
    async fn a_pinned_client_is_built_only_for_public_targets() {
        for url in ["https://127.0.0.1/x", "https://10.0.0.1/x", "https://[::1]/x", "ftp://example.com/x", "https://localhost/x"] {
            assert!(matches!(pinned_public_client(url, std::time::Duration::from_secs(1)).await, Err(UrlCheckError::Refused(_))), "{url}");
        }
        assert!(pinned_public_client("https://93.184.216.34/x", std::time::Duration::from_secs(1)).await.is_ok(), "a public literal needs no lookup");
    }
}
