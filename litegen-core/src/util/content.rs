//! Content helpers every layer uses on stored and fetched bytes (moved from
//! proxy/router.rs, docs/superpowers/specs/2026-11-07-engine-boundary-design.md D6).

/// An image's content type read from its first bytes (PixelGenie L9): PNG,
/// JPEG, WebP or GIF, else `None` and the adapter's label stands.
pub fn image_content_type(bytes: &[u8]) -> Option<&'static str> {
    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        Some("image/png")
    } else if bytes.starts_with(&[0xff, 0xd8, 0xff]) {
        Some("image/jpeg")
    } else if bytes.len() >= 12 && &bytes[..4] == b"RIFF" && &bytes[8..12] == b"WEBP" {
        Some("image/webp")
    } else if bytes.starts_with(b"GIF87a") || bytes.starts_with(b"GIF89a") {
        Some("image/gif")
    } else {
        None
    }
}

/// Lowercase hex SHA-256: what every stored asset reports, so a client can
/// check what it downloaded.
pub fn sha256_hex(bytes: &[u8]) -> String {
    use sha2::Digest;
    hex::encode(sha2::Sha256::digest(bytes))
}
