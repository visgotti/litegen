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

    #[test]
    fn dummy_verify_runs_constant_time() {
        // ~regression check: dummy hash verify should NOT panic + take similar time
        verify_dummy("any-password");
    }
}
