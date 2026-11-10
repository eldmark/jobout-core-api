//! Encryption of interview turn text at rest (case E7).
//!
//! `interview_turns.transcript` is a `bytea` column that only ever holds the output of
//! [`encrypt_turn_text`]: AES-256-GCM with a random nonce (`nonce ‖ ciphertext ‖ tag`), done in the
//! application before the text reaches PostgreSQL. It reuses [`PiiKeys`], so there is a single
//! encryption key to manage. Nothing here logs or formats the text.

use crate::crypto::{CryptoError, PiiKeys};

/// Encrypt the text of one turn. A fresh random nonce is used on every call, so encrypting the
/// same text twice gives different bytes.
pub fn encrypt_turn_text(keys: &PiiKeys, text: &str) -> Result<Vec<u8>, CryptoError> {
    keys.encrypt(text)
}

/// Decrypt what [`encrypt_turn_text`] produced. Fails if the bytes were altered, truncated, or
/// encrypted with another key.
pub fn decrypt_turn_text(keys: &PiiKeys, stored: &[u8]) -> Result<String, CryptoError> {
    keys.decrypt(stored)
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::{engine::general_purpose::STANDARD, Engine};

    fn keys(a: u8, b: u8) -> PiiKeys {
        PiiKeys::from_base64(&STANDARD.encode([a; 32]), &STANDARD.encode([b; 32])).unwrap()
    }

    #[test]
    fn round_trips_text_including_unicode_and_empty() {
        let k = keys(1, 2);
        for text in [
            "",
            "hola",
            "Trabajé en Mercado Libre — 3 años ñandú 🚀",
            &"x".repeat(10_000),
        ] {
            let stored = encrypt_turn_text(&k, text).unwrap();
            assert_eq!(decrypt_turn_text(&k, &stored).unwrap(), text);
        }
    }

    #[test]
    fn stored_bytes_do_not_contain_the_plaintext() {
        let k = keys(1, 2);
        let text = "trabajo en Mercado Libre";
        let stored = encrypt_turn_text(&k, text).unwrap();
        assert!(!stored.windows(text.len()).any(|w| w == text.as_bytes()));
        assert!(!stored.windows(7).any(|w| w == b"Mercado"));
    }

    #[test]
    fn nonce_is_random_so_same_text_encrypts_differently() {
        let k = keys(1, 2);
        let a = encrypt_turn_text(&k, "igual").unwrap();
        let b = encrypt_turn_text(&k, "igual").unwrap();
        assert_ne!(a, b);
        assert_ne!(a[..12], b[..12], "the nonce must differ");
        assert_eq!(decrypt_turn_text(&k, &a).unwrap(), "igual");
        assert_eq!(decrypt_turn_text(&k, &b).unwrap(), "igual");
    }

    #[test]
    fn flipping_any_byte_is_detected() {
        let k = keys(1, 2);
        let stored = encrypt_turn_text(&k, "respuesta").unwrap();
        for i in 0..stored.len() {
            let mut tampered = stored.clone();
            tampered[i] ^= 0x01;
            assert_eq!(
                decrypt_turn_text(&k, &tampered),
                Err(CryptoError::DecryptionFailed),
                "byte {i}"
            );
        }
    }

    #[test]
    fn truncated_or_empty_input_fails() {
        let k = keys(1, 2);
        let stored = encrypt_turn_text(&k, "respuesta").unwrap();
        assert!(decrypt_turn_text(&k, &stored[..stored.len() - 1]).is_err());
        assert!(decrypt_turn_text(&k, &stored[..10]).is_err());
        assert!(decrypt_turn_text(&k, &[]).is_err());
    }

    #[test]
    fn another_key_cannot_decrypt() {
        let stored = encrypt_turn_text(&keys(1, 2), "secreto").unwrap();
        assert_eq!(
            decrypt_turn_text(&keys(3, 4), &stored),
            Err(CryptoError::DecryptionFailed)
        );
    }
}
