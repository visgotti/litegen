use rand::RngCore;
use subtle::ConstantTimeEq;

pub fn generate_session_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rngs::OsRng.fill_bytes(&mut bytes);
    hex::encode(bytes)
}

pub fn generate_csrf_token() -> String {
    generate_session_token()
}

pub fn constant_time_eq(a: &str, b: &str) -> bool {
    let a = a.as_bytes();
    let b = b.as_bytes();
    let max = a.len().max(b.len());
    // Pad both slices to the same length so the ct_eq work is always done in
    // constant time regardless of input lengths.  The explicit length check at
    // the end is NOT a timing leak: by that point the constant-time compare has
    // already run unconditionally.
    let mut a_padded = a.to_vec();
    let mut b_padded = b.to_vec();
    a_padded.resize(max, 0);
    b_padded.resize(max, 0);
    a_padded.ct_eq(&b_padded).into() && a.len() == b.len()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn session_token_is_64_hex_chars() {
        let t = generate_session_token();
        assert_eq!(t.len(), 64);
        assert!(t.chars().all(|c| c.is_ascii_hexdigit()));
    }

    #[test]
    fn two_tokens_differ() {
        assert_ne!(generate_session_token(), generate_session_token());
    }

    #[test]
    fn csrf_token_is_64_hex_chars() {
        let t = generate_csrf_token();
        assert_eq!(t.len(), 64);
    }

    #[test]
    fn constant_time_compare_works() {
        assert!(constant_time_eq("abc", "abc"));
        assert!(!constant_time_eq("abc", "abd"));
        assert!(!constant_time_eq("abc", "abcd"));
    }
}
