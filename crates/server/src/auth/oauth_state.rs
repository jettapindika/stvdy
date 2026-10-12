//! The encrypted OAuth `state`, shared by the two flows that need one.
//!
//! Extracted from `auth::google` when the calendar consent flow arrived (A235): the login
//! flow and the calendar connect flow both carry a **PKCE verifier** through a browser round
//! trip, and a second copy of an AES-GCM `state` codec is a second place to get the nonce
//! handling wrong. The login flow's version has the tests, so the login flow's version is the
//! one that moved.
//!
//! Two properties this module exists to keep:
//!
//!  * **The `state` is encrypted, not signed.** It carries the PKCE verifier, and a
//!    signed-but-readable `state` would hand that verifier to the browser — the party PKCE
//!    exists to distrust. AES-GCM under `ENCRYPTION_KEY_HEX` keeps it server-side while
//!    staying stateless, so it works with more than one instance.
//!  * **The two flows share a codec, not a trust domain.** `purpose` is inside the encrypted
//!    payload and is checked on decrypt, so a `state` minted by the login flow cannot be
//!    replayed at the calendar callback. Without it, a login `state` would be accepted by the
//!    calendar flow (and vice versa), and the callback would act on a payload whose
//!    `user_id` the other flow never set.

use aes_gcm::aead::{Aead, KeyInit};
use aes_gcm::{Aes256Gcm, Nonce};
use base64::Engine;
use chrono::Utc;
use rand::Rng;
use serde::{Deserialize, Serialize};
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// How long a `state` (and therefore a PKCE verifier) stays valid.
pub const STATE_TTL_SECONDS: i64 = 600;

/// Which flow minted a `state`.
///
/// Serialised by name rather than by discriminant: the payload is JSON inside the ciphertext,
/// and a name survives a variant being inserted in the middle of the enum.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Purpose {
    /// `GET /auth/google/start` → `GET /auth/google/callback`.
    Login,
    /// `GET /calendar/google/connect` → `GET /calendar/google/callback`.
    Calendar,
}

/// What travels in `state`. Everything here is sensitive: the verifier is a credential and the
/// nonce is what binds an `id_token` to this request.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct StatePayload {
    /// Which flow minted this, checked on decrypt.
    pub purpose: Purpose,
    pub verifier: String,
    /// The `nonce` sent to Google. Only the login flow reads it back (the calendar flow asks
    /// for no `id_token`), but it is always minted: a payload whose shape varies by purpose
    /// would make the two flows' `state` distinguishable to anyone who guessed the key, and
    /// an always-present field is one less branch in the codec.
    pub nonce: String,
    pub exp: i64,
    /// The signed-in account a calendar connect belongs to.
    ///
    /// `None` for a login, which is the whole point: a login flow has no user yet — resolving
    /// one is what it does — while the calendar flow is started from an authenticated request
    /// and must return the grant to *that* account. Carrying it here rather than in a server
    /// session keeps the flow stateless.
    pub user_id: Option<Uuid>,
    /// The device the app declared, carried through the browser round trip so the login
    /// callback can create it. Unused by the calendar flow.
    pub device_name: String,
    pub device_platform: String,
    pub app_version: Option<String>,
}

/// The AES-GCM cipher the two flows share, keyed from `ENCRYPTION_KEY_HEX`.
pub fn cipher(state: &AppState) -> AppResult<Aes256Gcm> {
    let key = hex::decode(&state.config.encryption_key_hex)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("ENCRYPTION_KEY_HEX: {error}")))?;
    Aes256Gcm::new_from_slice(&key)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("encryption key length: {error}")))
}

/// Encrypts a payload into the URL-safe `state` parameter.
///
/// The 12-byte nonce is random per call and prepended to the ciphertext, which is what makes
/// two `state` values for the same payload differ: a deterministic nonce with one key is the
/// failure AES-GCM does not survive.
pub fn encrypt_state(state: &AppState, payload: &StatePayload) -> AppResult<String> {
    let plaintext = serde_json::to_vec(payload)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("state payload: {error}")))?;
    let mut nonce_bytes = [0u8; 12];
    rand::rng().fill_bytes(&mut nonce_bytes);
    let nonce = Nonce::from(nonce_bytes);

    let ciphertext = cipher(state)?
        .encrypt(&nonce, plaintext.as_ref())
        .map_err(|_| AppError::Internal(anyhow::anyhow!("encrypting the OAuth state")))?;

    let mut combined = nonce_bytes.to_vec();
    combined.extend_from_slice(&ciphertext);
    Ok(base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(combined))
}

/// Decrypts a `state` and refuses anything that is not this flow's.
///
/// Three refusals, and each is a different attack: a body that does not authenticate (a
/// tampered or forged `state`), a payload minted by the *other* flow (a replay across flows),
/// and an expired one (a captured URL replayed later). All three are `Validation`, because the
/// remedy for each is "start again" rather than "sign in".
pub fn decrypt_state(
    state: &AppState,
    expected: Purpose,
    encoded: &str,
) -> AppResult<StatePayload> {
    let invalid = || AppError::Validation("the OAuth state is not valid".to_string());

    let raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(encoded)
        .map_err(|_| invalid())?;
    // 12 bytes of nonce plus at least a tag: anything shorter cannot be a ciphertext.
    if raw.len() < 13 {
        return Err(invalid());
    }
    let (nonce_bytes, ciphertext) = raw.split_at(12);
    let nonce: [u8; 12] = nonce_bytes.try_into().map_err(|_| invalid())?;
    let nonce = Nonce::from(nonce);

    let plaintext = cipher(state)?
        .decrypt(&nonce, ciphertext)
        .map_err(|_| invalid())?;

    let payload: StatePayload = serde_json::from_slice(&plaintext).map_err(|_| invalid())?;

    if payload.purpose != expected {
        // Deliberately the same message as a forged state: telling a caller which flow a
        // `state` belongs to would confirm a valid one exists.
        return Err(invalid());
    }
    if payload.exp < Utc::now().timestamp() {
        return Err(AppError::Validation(
            "this sign-in attempt expired; start again".to_string(),
        ));
    }
    Ok(payload)
}

/// A PKCE verifier and its `S256` challenge (RFC 7636).
pub fn pkce_pair() -> (String, String) {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    let verifier = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
    let challenge = {
        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(verifier.as_bytes());
        base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize())
    };
    (verifier, challenge)
}

/// 32 random bytes, URL-safe base64 with no padding.
pub fn random_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// The SHA-256 hex digest of a value, for a column that stores a credential's hash.
pub fn hash(value: &str) -> String {
    use sha2::{Digest, Sha256};
    let mut hasher = Sha256::new();
    hasher.update(value.as_bytes());
    hex::encode(hasher.finalize())
}

/// When a `state` minted now expires, as a unix timestamp.
pub fn state_expiry() -> i64 {
    Utc::now().timestamp() + STATE_TTL_SECONDS
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::testing::TestState;
    use sqlx::PgPool;

    /// A state round-trips, a flipped byte does not decrypt, and **the other flow's state is
    /// refused** — which is the property the two flows sharing a codec makes necessary.
    #[sqlx::test(migrations = "../../migrations")]
    async fn a_state_round_trips_and_is_bound_to_its_flow(pool: PgPool) {
        let harness = TestState::new(pool);
        let state = &harness.state;

        let login = StatePayload {
            purpose: Purpose::Login,
            verifier: "v".to_string(),
            nonce: "n".to_string(),
            exp: state_expiry(),
            user_id: None,
            device_name: "laptop".to_string(),
            device_platform: "macos".to_string(),
            app_version: None,
        };
        let encoded = encrypt_state(state, &login).expect("encrypt");

        // The login flow accepts its own.
        let back = decrypt_state(state, Purpose::Login, &encoded).expect("decrypt");
        assert_eq!(back.purpose, Purpose::Login);
        assert_eq!(back.verifier, "v");
        assert!(back.user_id.is_none());

        // The calendar flow must not: a login state has no `user_id`, and acting on it would
        // connect a calendar to whatever the payload's owner turned out to be.
        assert!(
            decrypt_state(state, Purpose::Calendar, &encoded).is_err(),
            "a state minted by the login flow must not be accepted by the calendar flow"
        );

        // A calendar state carries its account, and the login flow refuses it too.
        let calendar = StatePayload {
            purpose: Purpose::Calendar,
            user_id: Some(uuid::Uuid::now_v7()),
            ..login
        };
        let encoded = encrypt_state(state, &calendar).expect("encrypt");
        let back = decrypt_state(state, Purpose::Calendar, &encoded).expect("decrypt");
        assert_eq!(back.user_id, calendar.user_id);
        assert!(decrypt_state(state, Purpose::Login, &encoded).is_err());

        // A tampered byte fails to authenticate rather than producing different claims.
        let mut raw = base64::engine::general_purpose::URL_SAFE_NO_PAD
            .decode(&encoded)
            .expect("decode");
        let last = raw.len() - 1;
        raw[last] ^= 0x01;
        let tampered = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(&raw);
        assert!(decrypt_state(state, Purpose::Calendar, &tampered).is_err());

        // An expired state is refused, and says so: the remedy is to start again.
        let expired = StatePayload {
            exp: Utc::now().timestamp() - 1,
            ..calendar
        };
        let encoded = encrypt_state(state, &expired).expect("encrypt");
        let refused = decrypt_state(state, Purpose::Calendar, &encoded);
        assert!(refused.is_err());
    }

    /// The PKCE pair is `S256` of a fresh verifier: a fixed verifier would make a captured
    /// authorization code replayable, and a plain challenge would be the verifier itself.
    #[test]
    fn pkce_uses_s256_and_a_fresh_verifier() {
        let (a, challenge_a) = pkce_pair();
        let (b, challenge_b) = pkce_pair();
        assert_ne!(a, b);
        assert_ne!(challenge_a, challenge_b);

        use sha2::{Digest, Sha256};
        let mut hasher = Sha256::new();
        hasher.update(a.as_bytes());
        let expected = base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(hasher.finalize());
        assert_eq!(challenge_a, expected);
        // The verifier is the pre-image, so the challenge must never equal it.
        assert_ne!(challenge_a, a);
    }

    /// A token is 32 bytes of base64url, and two calls differ. The feed token's unguessability
    /// rests on this being a real CSPRNG draw rather than a counter.
    #[test]
    fn a_random_token_is_long_and_fresh() {
        let first = random_token();
        let second = random_token();
        assert_ne!(first, second);
        assert_eq!(first.len(), 43, "32 bytes base64url, unpadded");
        assert!(
            first
                .chars()
                .all(|c| c.is_ascii_alphanumeric() || c == '-' || c == '_')
        );
        assert_ne!(hash(&first), hash(&second));
    }

    /// Two encryptions of one payload differ, so a captured `state` is not a value an attacker
    /// can predict or replay from a previous attempt.
    #[sqlx::test(migrations = "../../migrations")]
    async fn the_nonce_is_fresh_per_call(pool: PgPool) {
        let harness = TestState::new(pool);
        let payload = StatePayload {
            purpose: Purpose::Login,
            verifier: "v".to_string(),
            nonce: "n".to_string(),
            exp: state_expiry(),
            user_id: None,
            device_name: "d".to_string(),
            device_platform: "web".to_string(),
            app_version: None,
        };
        let first = encrypt_state(&harness.state, &payload).expect("encrypt");
        let second = encrypt_state(&harness.state, &payload).expect("encrypt");
        assert_ne!(first, second, "the nonce must be fresh per call");
        assert_eq!(
            decrypt_state(&harness.state, Purpose::Login, &first)
                .expect("decrypt")
                .verifier,
            "v"
        );
        assert_eq!(
            decrypt_state(&harness.state, Purpose::Login, &second)
                .expect("decrypt")
                .verifier,
            "v"
        );
    }
}
