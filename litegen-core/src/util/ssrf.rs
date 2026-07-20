//! SSRF guard for server-side outbound fetches.
//!
//! Used by features that fetch a *user-supplied* URL from the server
//! (reference-image materialization, webhook dispatch). It rejects URLs whose
//! scheme is not http(s) or whose host resolves to a non-public address
//! (loopback, private, link-local — including the `169.254.169.254` cloud
//! metadata endpoint —, unique-local, CGNAT, etc.).
//!
//! NOTE on residual risk: there is a DNS-rebinding window because the HTTP
//! client re-resolves the host when it connects, after this check. Callers
//! MUST therefore also disable HTTP redirect following (e.g.
//! `reqwest::redirect::Policy::none()`) so that a public host cannot issue a
//! 3xx redirect into a private target, and should keep the rebinding window
//! small. This guard plus no-redirects closes the practical SSRF vectors.

use std::net::{IpAddr, Ipv4Addr};

/// True if `ip` must never be the target of a server-side fetch.
pub fn is_disallowed_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(v4) => {
            v4.is_private()
                || v4.is_loopback()
                || v4.is_link_local() // includes 169.254.169.254 metadata
                || v4.is_unspecified()
                || v4.is_broadcast()
                || v4.is_documentation()
                || v4.octets()[0] == 0 // 0.0.0.0/8 "this host"
                // 100.64.0.0/10 carrier-grade NAT
                || (v4.octets()[0] == 100 && (v4.octets()[1] & 0xc0) == 0x40)
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
            v6.is_loopback()
                || v6.is_unspecified()
                // fc00::/7 unique local
                || (v6.octets()[0] & 0xfe) == 0xfc
                // fe80::/10 link local
                || (v6.octets()[0] == 0xfe && (v6.octets()[1] & 0xc0) == 0x80)
        }
    }
}

/// Validate that `url` is safe to fetch server-side. Returns `Err(reason)` if
/// the scheme is unsupported, the host is missing, fails to resolve, or
/// resolves to any disallowed address.
pub async fn validate_public_url(url: &str) -> Result<(), String> {
    let parsed = reqwest::Url::parse(url).map_err(|e| format!("invalid url: {e}"))?;

    match parsed.scheme() {
        "http" | "https" => {}
        other => return Err(format!("unsupported url scheme: {other}")),
    }

    let host = parsed.host_str().ok_or_else(|| "url has no host".to_string())?;
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
            return Err("url targets a disallowed (private/loopback/link-local) address".to_string());
        }
        return Ok(());
    }

    // Otherwise it's a hostname: resolve it and check every address it maps to.
    let mut any = false;
    let addrs = tokio::net::lookup_host((host, port))
        .await
        .map_err(|e| format!("dns resolution failed: {e}"))?;
    for addr in addrs {
        any = true;
        if is_disallowed_ip(addr.ip()) {
            return Err("url resolves to a disallowed (private/loopback/link-local) address".to_string());
        }
    }
    if !any {
        return Err("host did not resolve".to_string());
    }
    Ok(())
}

/// Build a `reqwest::Client` hardened for server-side fetches of user-supplied
/// or user-influenced URLs: it does NOT follow redirects, so a public host can't
/// 3xx-redirect a request into an internal target after `validate_public_url`
/// has checked the original URL (see the DNS-rebinding note above). Falls back to
/// a default client only if the builder somehow fails. Use this anywhere the
/// server fetches a webhook/reference URL rather than hand-rolling the builder.
/// Total-request timeout for server-side fetches of user-supplied URLs. A
/// hostile or hung host must never be able to pin a webhook/reference-fetch task
/// (and its connection) open forever.
const FETCH_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);
/// Connect-phase timeout (a subset of the total), so a host that accepts the TCP
/// SYN but never completes the handshake is bounded too.
const CONNECT_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(10);

pub fn no_redirect_client() -> reqwest::Client {
    no_redirect_client_with_timeout(FETCH_TIMEOUT)
}

/// Same as [`no_redirect_client`] but with a caller-chosen total timeout. Kept
/// separate so tests can exercise the timeout behaviour with a short deadline.
pub fn no_redirect_client_with_timeout(timeout: std::time::Duration) -> reqwest::Client {
    reqwest::Client::builder()
        .redirect(reqwest::redirect::Policy::none())
        .timeout(timeout)
        .connect_timeout(CONNECT_TIMEOUT)
        .build()
        .unwrap_or_else(|_| reqwest::Client::new())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::net::{Ipv4Addr, Ipv6Addr};

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
}
