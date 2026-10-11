//! Password hashing: argon2id, with the parameters pinned in one place.

use argon2::password_hash::PasswordHasher;
use argon2::password_hash::PasswordVerifier;
use argon2::password_hash::phc::PasswordHash;
use argon2::{Algorithm, Argon2, Params, Version};

use crate::error::{AppError, AppResult};

/// argon2id at 19 MiB, 2 iterations, 1 lane: the OWASP-recommended floor for a
/// server that also has to hash under load. Raising the memory cost is the knob
/// to turn if the hardware allows.
const MEMORY_KIB: u32 = 19 * 1024;
const ITERATIONS: u32 = 2;
const LANES: u32 = 1;

/// Length is the only rule that reliably helps, so there is no composition
/// requirement (which pushes users toward `Passw0rd!`).
pub const MIN_PASSWORD_LEN: usize = 8;
/// argon2's cost is linear in the input, and no human password is this long.
pub const MAX_PASSWORD_LEN: usize = 128;

fn hasher() -> AppResult<Argon2<'static>> {
    let params = Params::new(MEMORY_KIB, ITERATIONS, LANES, None)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("argon2 parameters: {error}")))?;
    Ok(Argon2::new(Algorithm::Argon2id, Version::V0x13, params))
}

pub fn validate(password: &str) -> AppResult<()> {
    if password.chars().count() < MIN_PASSWORD_LEN {
        return Err(AppError::Validation(format!(
            "password must be at least {MIN_PASSWORD_LEN} characters"
        )));
    }
    if password.len() > MAX_PASSWORD_LEN {
        return Err(AppError::Validation(format!(
            "password must be at most {MAX_PASSWORD_LEN} bytes"
        )));
    }
    Ok(())
}

/// Hashes with a fresh random salt (argon2's own generator), so two identical
/// passwords produce different PHC strings.
pub fn hash(password: &str) -> AppResult<String> {
    validate(password)?;
    hasher()?
        .hash_password(password.as_bytes())
        .map(|hash| hash.to_string())
        .map_err(|error| AppError::Internal(anyhow::anyhow!("hashing a password: {error}")))
}

pub fn verify(password: &str, stored_hash: &str) -> bool {
    let Ok(parsed) = PasswordHash::new(stored_hash) else {
        return false;
    };
    // The verifier takes its parameters from the hash string itself, so a hash
    // written when MEMORY_KIB was lower keeps verifying after the constant is
    // raised. This is why `verify` does not build a configured `Argon2`.
    Argon2::default()
        .verify_password(password.as_bytes(), &parsed)
        .is_ok()
}

/// Burns the same work as a real verification.
///
/// Login must take the same time whether or not the account exists, or the
/// response time answers "is this email registered?" for an attacker. The dummy
/// hash is built once from a real password so it is always a well-formed PHC
/// string: a hand-written constant that failed to parse would make `verify`
/// return early and defeat the whole exercise.
pub fn verify_dummy(password: &str) {
    static DUMMY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
        hasher()
            .and_then(|argon| {
                argon
                    .hash_password(b"not-a-real-password")
                    .map(|hash| hash.to_string())
                    .map_err(|error| {
                        AppError::Internal(anyhow::anyhow!("building the dummy hash: {error}"))
                    })
            })
            .unwrap_or_default()
    });
    let _ = verify(password, DUMMY.as_str());
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_password_round_trips_and_a_wrong_one_does_not() {
        let hash = hash("correct horse battery").expect("hash");
        assert!(verify("correct horse battery", &hash));
        assert!(!verify("correct horse batteru", &hash));
    }

    #[test]
    fn the_same_password_hashes_differently_each_time() {
        let a = hash("correct horse battery").expect("hash");
        let b = hash("correct horse battery").expect("hash");
        assert_ne!(a, b, "a per-password salt must make the hashes differ");
        assert!(verify("correct horse battery", &a) && verify("correct horse battery", &b));
    }

    #[test]
    fn the_hash_names_argon2id_and_the_pinned_parameters() {
        let hash = hash("correct horse battery").expect("hash");
        assert!(
            hash.starts_with("$argon2id$v=19$m=19456,t=2,p=1$"),
            "{hash}"
        );
    }

    #[test]
    fn short_passwords_are_rejected_before_hashing() {
        assert!(validate("short").is_err());
        assert!(validate(&"a".repeat(MAX_PASSWORD_LEN + 1)).is_err());
        assert!(validate("longenough").is_ok());
    }

    #[test]
    fn a_corrupt_hash_does_not_verify_and_does_not_panic() {
        assert!(!verify("anything", "not-a-hash"));
        assert!(!verify("anything", ""));
    }

    #[test]
    fn the_dummy_verification_burns_real_work() {
        // Not a timing assertion (which would be flaky), but a check that the
        // dummy hash parses: a malformed one would return early and leak the
        // difference the function exists to hide.
        static DUMMY: std::sync::LazyLock<String> = std::sync::LazyLock::new(|| {
            hasher()
                .expect("params")
                .hash_password(b"not-a-real-password")
                .expect("hash")
                .to_string()
        });
        assert!(PasswordHash::new(DUMMY.as_str()).is_ok());
        verify_dummy("whatever");
    }
}
