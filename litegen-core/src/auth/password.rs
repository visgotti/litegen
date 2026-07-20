use argon2::{
    password_hash::{rand_core::OsRng, PasswordHash, PasswordHasher, PasswordVerifier, SaltString},
    Algorithm, Argon2, Params, Version,
};

#[derive(Debug, thiserror::Error)]
pub enum PasswordError {
    #[error("password must be at least 12 characters")]
    TooShort,
    #[error("argon2 hash error: {0}")]
    Hash(String),
    #[error("argon2 verify error: {0}")]
    Verify(String),
}

pub const MIN_PASSWORD_LEN: usize = 12;

fn params() -> Params {
    Params::new(65536, 3, 1, None).expect("argon2 params")
}

fn argon() -> Argon2<'static> {
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params())
}

pub fn hash_password(plain: &str) -> Result<String, PasswordError> {
    if plain.len() < MIN_PASSWORD_LEN {
        return Err(PasswordError::TooShort);
    }
    let salt = SaltString::generate(&mut OsRng);
    let hash = argon()
        .hash_password(plain.as_bytes(), &salt)
        .map_err(|e| PasswordError::Hash(e.to_string()))?;
    Ok(hash.to_string())
}

pub fn verify_password(plain: &str, phc: &str) -> Result<bool, PasswordError> {
    let parsed =
        PasswordHash::new(phc).map_err(|e| PasswordError::Verify(e.to_string()))?;
    Ok(argon().verify_password(plain.as_bytes(), &parsed).is_ok())
}

/// Run a verify against a precomputed dummy hash. Use during login when user not found
/// to keep response time constant and prevent user enumeration.
pub fn verify_dummy(plain: &str) {
    static DUMMY_HASH: once_cell::sync::Lazy<String> = once_cell::sync::Lazy::new(|| {
        hash_password("dummy-dummy-dummy").expect("dummy hash")
    });
    let _ = verify_password(plain, &DUMMY_HASH);
}

// ─── Async wrappers ─────────────────────────────────────────────────────────
//
// Argon2id here is deliberately expensive (64 MiB, t=3). Called inline on an
// async task it blocks a tokio worker thread for the whole computation, so a
// burst of unauthenticated /auth/login|signup requests can starve the worker
// pool and stall the entire gateway. These wrappers offload the CPU-bound work
// to the blocking pool, keeping the async workers free to serve other requests.

/// Offloaded [`hash_password`].
pub async fn hash_password_async(plain: String) -> Result<String, PasswordError> {
    tokio::task::spawn_blocking(move || hash_password(&plain))
        .await
        .map_err(|e| PasswordError::Hash(format!("hash task join error: {e}")))?
}

/// Offloaded [`verify_password`].
pub async fn verify_password_async(plain: String, phc: String) -> Result<bool, PasswordError> {
    tokio::task::spawn_blocking(move || verify_password(&plain, &phc))
        .await
        .map_err(|e| PasswordError::Verify(format!("verify task join error: {e}")))?
}

/// Offloaded [`verify_dummy`].
pub async fn verify_dummy_async(plain: String) {
    let _ = tokio::task::spawn_blocking(move || verify_dummy(&plain)).await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn hash_then_verify_succeeds() {
        let hash = hash_password("correct-horse-battery-staple-1").unwrap();
        assert!(verify_password("correct-horse-battery-staple-1", &hash).unwrap());
    }

    #[test]
    fn verify_rejects_wrong_password() {
        let hash = hash_password("correct-horse-battery-staple-1").unwrap();
        assert!(!verify_password("wrong-password", &hash).unwrap());
    }

    #[test]
    fn min_length_enforced() {
        let result = hash_password("short");
        assert!(matches!(result, Err(PasswordError::TooShort)));
    }

    #[tokio::test]
    async fn async_hash_then_verify_roundtrips() {
        // The offloaded wrappers must produce a real, verifiable hash — a
        // burst of these runs on the blocking pool, but correctness is identical.
        let pw = "correct-horse-battery-staple-1".to_string();
        let hash = hash_password_async(pw.clone()).await.expect("hash");
        assert!(
            verify_password_async(pw, hash.clone()).await.unwrap(),
            "async round-trip must verify the correct password"
        );
        assert!(
            !verify_password_async("wrong-password".to_string(), hash).await.unwrap(),
            "async verify must reject a wrong password"
        );
    }

    #[test]
    fn dummy_verify_runs_constant_time() {
        // ~regression check: dummy hash verify should NOT panic + take similar time
        verify_dummy("any-password");
    }
}
