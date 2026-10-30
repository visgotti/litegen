//! Downloading a vendor's result files: what every 3D adapter does with the
//! URLs a finished job names, so the bytes can be re-hosted before those URLs
//! expire (several vendors' do within minutes).

use crate::providers::ProviderError;

/// Why an asset did not download.
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
    if public_only {
        crate::util::ssrf::check_public_url(url).await.map_err(|why| match why {
            // A resolver blip is not a verdict on the URL.
            crate::util::ssrf::UrlCheckError::Lookup(e) => AssetError::Transient(ProviderError::RequestFailed {
                message: format!("{vendor} asset host lookup failed: {e}"),
                status_code: None,
                provider_error: None,
                retryable: true,
            }),
            crate::util::ssrf::UrlCheckError::Refused(e) => AssetError::Gone(format!("asset URL refused: {e}")),
        })?;
    }
    let mut resp = client.get(url).send().await.map_err(|e| {
        AssetError::Transient(ProviderError::RequestFailed {
            message: format!("{vendor} asset download failed: {e}"),
            status_code: None,
            provider_error: None,
            retryable: true,
        })
    })?;
    let status = resp.status();
    if !status.is_success() {
        let message = format!("{vendor} asset download returned HTTP {}", status.as_u16());
        return Err(if transient(status) {
            AssetError::Transient(ProviderError::RequestFailed {
                message,
                status_code: Some(status.as_u16()),
                provider_error: None,
                retryable: true,
            })
        } else {
            AssetError::Gone(message)
        });
    }
    if let Some(len) = resp.content_length().filter(|&l| l > limit) {
        return Err(AssetError::Gone(format!("asset is {len} bytes, over the {limit}-byte limit")));
    }
    let content_type = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .and_then(|ct| ct.split(';').next())
        .map(|ct| ct.trim().to_string())
        .filter(|ct| !ct.is_empty())
        .unwrap_or_else(|| "application/octet-stream".to_string());
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
