//! Password hashing, so a credential never lives in the clear.
//!
//! The UO protocol sends the password as plaintext inside a login stream whose
//! encryption is trivially broken (see [`crate::accounts`]). That is the
//! client's fault and cannot be fixed server-side. What *can* be fixed is what
//! happens next: the credential a store keeps is a slow argon2 hash, and the
//! plaintext is compared against it and then dropped.
//!
//! The hash is a self-describing PHC string (`$argon2id$v=19$...`), so the
//! parameters travel with it and an old hash still verifies after a parameter
//! change. The salt is drawn from the OS entropy pool through `getrandom`, the
//! same source the auth keys use, so no PRNG feature is pulled in for it.

use argon2::password_hash::{PasswordHash, PasswordHasher, PasswordVerifier, SaltString};
use argon2::Argon2;

/// Hash a plaintext password into a PHC string for storage.
///
/// # Panics
///
/// Only if the OS entropy pool is unavailable, which is not a condition a shard
/// can meaningfully continue past.
#[must_use]
pub fn hash(plaintext: &str) -> String {
    let mut salt_bytes = [0u8; 16];
    getrandom::getrandom(&mut salt_bytes).expect("the OS entropy pool is available");
    let salt = SaltString::encode_b64(&salt_bytes).expect("sixteen bytes is a valid salt");
    Argon2::default()
        .hash_password(plaintext.as_bytes(), &salt)
        .expect("argon2 does not fail on a valid salt and password")
        .to_string()
}

/// Check a plaintext password against a stored PHC hash.
///
/// A credential that does not parse as a hash (a corrupt or non-argon2 row)
/// verifies against nothing — it is never treated as a plaintext to compare, so
/// a store cannot be tricked into plaintext comparison by holding a bare string.
#[must_use]
pub fn verify(plaintext: &str, phc: &str) -> bool {
    match PasswordHash::new(phc) {
        Ok(parsed) => Argon2::default()
            .verify_password(plaintext.as_bytes(), &parsed)
            .is_ok(),
        Err(_) => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_hash_verifies_its_own_password() {
        let phc = hash("hunter2");
        assert!(verify("hunter2", &phc));
    }

    #[test]
    fn a_hash_rejects_the_wrong_password() {
        let phc = hash("hunter2");
        assert!(!verify("hunter3", &phc));
        assert!(!verify("", &phc));
    }

    #[test]
    fn the_hash_is_not_the_plaintext() {
        let phc = hash("hunter2");
        assert!(!phc.contains("hunter2"), "the plaintext must not survive");
        assert!(phc.starts_with("$argon2"), "a self-describing PHC string");
    }

    #[test]
    fn two_hashes_of_one_password_differ() {
        // Different salts, so the same password never yields the same hash — an
        // attacker cannot tell two accounts share a password from the rows.
        assert_ne!(hash("same"), hash("same"));
    }

    #[test]
    fn a_non_phc_credential_verifies_against_nothing() {
        assert!(!verify("plaintext", "plaintext"));
        assert!(!verify("", ""));
    }
}
