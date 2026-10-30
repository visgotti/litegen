//! Publishing uploaded references for vendors that fetch inputs only by URL
//! (PixelGenie L4).
//!
//! A caller with no public host sends its reference images as base64. Most
//! URL-taking vendors accept those inline as `data:` URIs; a few do not (Luma:
//! "You should upload and use your own cdn image urls, currently this is the
//! only way to pass an image"). For those, litegen stores the bytes and serves
//! them at `{public_base_url}/v1/refs/{token}`: unguessable, answering for an
//! hour at most, and kept past the submit — an asynchronous vendor fetches its
//! inputs once a runner picks the job up, not when litegen's request returns.

use base64::Engine;
use rand::RngCore;
use std::sync::Arc;
use std::time::Duration;

use crate::proxy::storage::{ImageStorage, ImageStoreError};

/// How long a published reference answers: past any vendor's fetch of a
/// queued job's input, and never more than an hour (L4).
pub const REF_URL_TTL: Duration = Duration::from_secs(3600);

/// Where published references live in the asset store.
pub const REFS_PREFIX: &str = "litegen/refs";

/// Publishes reference bytes at litegen's own URLs, and serves them back.
pub struct RefPublisher {
    store: Arc<dyn ImageStorage>,
    base_url: String,
    ttl: Duration,
}

impl RefPublisher {
    pub fn new(store: Arc<dyn ImageStorage>, base_url: String) -> Self {
        Self { store, base_url: base_url.trim_end_matches('/').to_string(), ttl: REF_URL_TTL }
    }

    /// The same publisher with another lifetime (tests).
    pub fn with_ttl(mut self, ttl: Duration) -> Self {
        self.ttl = ttl;
        self
    }

    /// Store `bytes` and answer the URL a vendor fetches them at. Refused when
    /// litegen's public address is not one the internet can reach: a vendor
    /// would fail to fetch it, after the submit.
    pub async fn publish(&self, bytes: &[u8], content_type: &str) -> Result<String, String> {
        if !is_public_origin(&self.base_url) {
            return Err(format!(
                "this model fetches its references by URL, and litegen has no public address to publish one at \
                 (server.public_base_url is {}); set it to litegen's public HTTPS origin, or send a public image URL",
                self.base_url
            ));
        }
        let expiry = chrono::Utc::now().timestamp() + self.ttl.as_secs() as i64;
        let mut random = [0u8; 32];
        rand::thread_rng().fill_bytes(&mut random);
        let ext = match content_type {
            "image/jpeg" => "jpg",
            "image/webp" => "webp",
            "image/gif" => "gif",
            _ => "png",
        };
        let token = format!("{expiry}-{}.{ext}", base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(random));
        self.store
            .put(&format!("{REFS_PREFIX}/{token}"), &bytes::Bytes::copy_from_slice(bytes), content_type)
            .await
            .map_err(|e| format!("the reference could not be stored for the vendor: {e}"))?;
        Ok(format!("{}/v1/refs/{token}", self.base_url))
    }

    /// The bytes behind a token, while it has not expired.
    pub async fn get(&self, token: &str) -> Result<Option<(bytes::Bytes, String)>, ImageStoreError> {
        let Some((expiry, rest)) = token.split_once('-') else { return Ok(None) };
        let Ok(expiry) = expiry.parse::<i64>() else { return Ok(None) };
        let well_formed = !rest.is_empty() && rest.chars().all(|c| c.is_ascii_alphanumeric() || matches!(c, '-' | '_' | '.')) && !rest.contains("..");
        if !well_formed || chrono::Utc::now().timestamp() >= expiry {
            return Ok(None);
        }
        self.store.get(&format!("{REFS_PREFIX}/{token}")).await
    }
}

/// Delete the published refs under a local asset root whose expiry (the
/// token's leading number) is at or before `now`, in Unix seconds. Returns how
/// many went. Best-effort, like the retention sweep that calls it.
pub fn sweep_expired_refs(root: &std::path::Path, now: i64) -> usize {
    let Ok(entries) = std::fs::read_dir(root.join(REFS_PREFIX)) else { return 0 };
    entries
        .flatten()
        .filter(|e| {
            let name = e.file_name();
            let expiry = name.to_str().and_then(|n| n.split_once('-')).and_then(|(x, _)| x.parse::<i64>().ok());
            expiry.is_some_and(|x| x <= now)
        })
        .filter(|e| std::fs::remove_file(e.path()).is_ok())
        .count()
}

/// Whether a vendor on the internet could reach `base`: not loopback, not a
/// private or link-local address, not a single-label or `.local` name.
fn is_public_origin(base: &str) -> bool {
    let Ok(url) = reqwest::Url::parse(base) else { return false };
    let Some(host) = url.host_str() else { return false };
    let host = host.trim_start_matches('[').trim_end_matches(']').to_ascii_lowercase();
    if let Ok(ip) = host.parse::<std::net::IpAddr>() {
        return match ip {
            std::net::IpAddr::V4(v4) => !(v4.is_loopback() || v4.is_private() || v4.is_link_local() || v4.is_unspecified()),
            std::net::IpAddr::V6(v6) => {
                let first = v6.segments()[0];
                !(v6.is_loopback() || v6.is_unspecified() || (first & 0xfe00) == 0xfc00 || (first & 0xffc0) == 0xfe80)
            }
        };
    }
    host.contains('.') && !(host.ends_with(".localhost") || host.ends_with(".local") || host.ends_with(".internal"))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn publisher(ttl: std::time::Duration) -> RefPublisher {
        let dir = tempfile::tempdir().unwrap().keep();
        let store: std::sync::Arc<dyn crate::proxy::storage::ImageStorage> =
            std::sync::Arc::new(crate::proxy::storage::LocalModel3dStorage::new(dir, "https://litegen.example.com".into()));
        RefPublisher::new(store, "https://litegen.example.com".into()).with_ttl(ttl)
    }

    #[tokio::test]
    async fn a_published_ref_expires_within_the_hour() {
        let p = publisher(REF_URL_TTL);
        let url = p.publish(b"\x89PNGbytes", "image/png").await.unwrap();
        let token = url.rsplit('/').next().unwrap();
        let expiry: i64 = token.split('-').next().unwrap().parse().unwrap();
        let now = chrono::Utc::now().timestamp();
        assert!(expiry > now && expiry <= now + 3600, "{expiry} vs {now}");
        assert!(p.get(token).await.unwrap().is_some());
    }

    #[tokio::test]
    async fn the_sweep_removes_expired_refs_and_keeps_live_ones() {
        let dir = tempfile::tempdir().unwrap().keep();
        let store: std::sync::Arc<dyn crate::proxy::storage::ImageStorage> =
            std::sync::Arc::new(crate::proxy::storage::LocalModel3dStorage::new(dir.clone(), "https://litegen.example.com".into()));
        let expired = RefPublisher::new(store.clone(), "https://litegen.example.com".into()).with_ttl(std::time::Duration::ZERO);
        let live = RefPublisher::new(store, "https://litegen.example.com".into());
        expired.publish(b"old", "image/png").await.unwrap();
        let keep = live.publish(b"new", "image/png").await.unwrap();
        // Something else under the asset root is not a ref, and stays.
        std::fs::create_dir_all(dir.join("litegen/3d/x")).unwrap();
        std::fs::write(dir.join("litegen/3d/x/model.glb"), b"glTF").unwrap();
        assert_eq!(sweep_expired_refs(&dir, chrono::Utc::now().timestamp()), 1);
        assert!(live.get(keep.rsplit('/').next().unwrap()).await.unwrap().is_some());
        assert!(dir.join("litegen/3d/x/model.glb").exists());
    }

    #[tokio::test]
    async fn an_expired_or_unknown_token_is_not_served() {
        let p = publisher(std::time::Duration::ZERO);
        let url = p.publish(b"bytes", "image/png").await.unwrap();
        let token = url.rsplit('/').next().unwrap();
        assert!(p.get(token).await.unwrap().is_none(), "expired");
        let live = publisher(REF_URL_TTL);
        for bad in ["9999999999-nope", "../../etc/passwd", "abc", ""] {
            assert!(live.get(bad).await.unwrap().is_none(), "{bad}");
        }
    }
}
