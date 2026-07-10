use sha2::{Digest, Sha256};

/// Characters for the random key body: unambiguous base62.
const ALPHABET: &[u8] = b"ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz0123456789";
/// 40 base62 characters ≈ 238 bits of entropy, comfortably above the
/// 192-bit floor required by the api-key-management spec.
const KEY_BODY_LEN: usize = 40;
/// Display prefix stored beside the hash so owners can recognize a key.
const PREFIX_LEN: usize = 8;

/// Generates a new plaintext API key: `la-` + 40 random base62 characters.
/// Returned exactly once at creation; only the SHA-256 hash is stored.
pub(crate) fn generate_api_key() -> String {
    use argon2::password_hash::rand_core::{OsRng, RngCore};

    let mut key = String::with_capacity(3 + KEY_BODY_LEN);
    key.push_str("la-");
    while key.len() < 3 + KEY_BODY_LEN {
        let mut bytes = [0u8; 64];
        OsRng.fill_bytes(&mut bytes);
        for byte in bytes {
            // Rejection sampling keeps the draw uniform across the alphabet.
            if usize::from(byte) < ALPHABET.len() * 4 {
                key.push(ALPHABET[usize::from(byte) % ALPHABET.len()] as char);
                if key.len() == 3 + KEY_BODY_LEN {
                    break;
                }
            }
        }
    }
    key
}

/// The lookup digest for a presented key. SHA-256 (not argon2) is deliberate:
/// keys are high-entropy random strings, so a fast deterministic digest is
/// safe and keeps the per-request lookup cheap.
pub(crate) fn hash_api_key(key: &str) -> [u8; 32] {
    Sha256::digest(key.as_bytes()).into()
}

/// Hex form of the lookup digest, as stored in the `api_keys.key_hash` column.
pub(crate) fn hash_api_key_hex(key: &str) -> String {
    hex(&hash_api_key(key))
}

pub(crate) fn hex(bytes: &[u8]) -> String {
    bytes.iter().fold(
        String::with_capacity(bytes.len() * 2),
        |mut out, byte| {
            use std::fmt::Write;
            let _ = write!(out, "{byte:02x}");
            out
        },
    )
}

/// The display prefix persisted for key recognition (`la-Ab3dE`-style).
pub(crate) fn key_prefix(key: &str) -> String {
    key.chars().take(PREFIX_LEN).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn generated_keys_have_expected_shape() {
        let key = generate_api_key();
        assert!(key.starts_with("la-"));
        assert_eq!(key.len(), 43);
        assert!(
            key[3..]
                .bytes()
                .all(|byte| ALPHABET.contains(&byte))
        );
    }

    #[test]
    fn generated_keys_are_unique() {
        let a = generate_api_key();
        let b = generate_api_key();
        assert_ne!(a, b);
    }

    #[test]
    fn prefix_is_first_eight_chars() {
        assert_eq!(key_prefix("la-abcdefghij"), "la-abcde");
    }

    #[test]
    fn hash_is_deterministic_and_hex_matches() {
        let key = "la-test";
        assert_eq!(hash_api_key(key), hash_api_key(key));
        assert_eq!(hash_api_key_hex(key), hex(&hash_api_key(key)));
        assert_eq!(hash_api_key_hex(key).len(), 64);
    }
}
