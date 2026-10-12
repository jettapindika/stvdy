//! BYOK: the student's own model key, and the two rules that keep it honest.
//!
//! Power only, web/desktop only (A302). The key is encrypted with the same `aes-gcm` cipher
//! and the same `ENCRYPTION_KEY_HEX` the OAuth state uses — through
//! [`crate::auth::oauth_state::cipher`], so there is one key reader rather than two — and it
//! is **never returned**: `GET /settings/api-key` answers with whether a key is set and which
//! endpoint and model it points at, and no route or log line carries the secret.
//!
//! The two rules, and each has a way of going wrong that a reader would not see:
//!
//!  * **A BYOK call consumes no credits** (A302). The student paid their own provider, so
//!    charging them an allowance for it would be charging twice. The seam is
//!    [`provider_for`], which the chat and generation paths call once per request: when it
//!    answers `Some`, the spend is skipped and the cost is still recorded with
//!    `usage_events.byok = true` through `credits::record_byok_usage`.
//!  * **The ceiling does not pause a BYOK account** (A156). The ceiling exists to bound *our*
//!    provider bill; a call on the student's key is not our cost, so pausing them for it
//!    would be pausing an account for spending someone else's money. `spend_in` already
//!    excludes `byok = true` from the sum it compares, and the BYOK path skips `spend_in`
//!    entirely — so the exclusion is belt and braces, which is what a money path wants.
//!
//! A stored key whose plan no longer grants BYOK is **left in place and ignored** rather than
//! deleted: a student who drops to Pro and comes back to Power should not have to re-enter a
//! key, and deleting a secret on a plan change is not something a downgrade should be able
//! to do.

use std::sync::Arc;

use aes_gcm::aead::Aead;
use aes_gcm::{Aes256Gcm, Nonce};
use serde::{Deserialize, Serialize};
use stvdy_core::config::PlanId;
use stvdy_core::entitlements::Feature;
use stvdy_core::traits::llm::LlmProvider;

use crate::auth::extract::AuthUser;
use crate::config::LlmConfig;
use crate::error::{AppError, AppResult};
use crate::integrations::llm::OpenAiCompatLlm;
use crate::state::AppState;

/// The provider a stored key belongs to. One value today: the key is an OpenAI-compatible
/// one, which is what `OpenAiCompatLlm` speaks (A1), and a provider name is stored so a
/// second shape is a data change rather than a schema change.
pub const PROVIDER: &str = "openai_compatible";

/// What the client is told about the account's key: **never the key**.
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ByokDescription {
    pub set: bool,
    pub provider: Option<String>,
    pub base_url: Option<String>,
    pub model: Option<String>,
}

/// The body of `PUT /settings/api-key`.
#[derive(Debug, Deserialize)]
pub struct ByokRequest {
    pub key: String,
    /// The endpoint, for a compatible provider that is not the default one. Falls back to
    /// the server's configured `LLM_BASE_URL` (A302).
    #[serde(default)]
    pub base_url: Option<String>,
    /// The model to ask for. Falls back to the server's configured model.
    #[serde(default)]
    pub model: Option<String>,
}

/// Stores (or replaces) the account's key, encrypted.
///
/// Two refusals before anything is written, and they are different rules: the platform one is
/// the *same* rule that refuses checkout (A302 — a phone cannot set a key, because the
/// billing policy keeps purchase-shaped actions off a phone), and the plan one is the feature
/// gate. Both run before the encrypt so a refused request leaves no ciphertext behind.
pub async fn set(
    state: &AppState,
    user: &AuthUser,
    body: ByokRequest,
) -> AppResult<ByokDescription> {
    crate::billing::checkout_allowed(&state.config.billing_policy, &user.platform)?;
    crate::entitlements::require(state, user, Feature::Byok).await?;

    let key = body.key.trim();
    if key.is_empty() {
        return Err(AppError::Validation("key must not be empty".to_string()));
    }

    // The endpoint and the model are the user's, and both are optional: `None` means "use the
    // server's", which is what makes a key for the default provider a one-field form.
    let base_url = normalized(body.base_url.as_deref(), "base_url")?;
    let model = normalized(body.model.as_deref(), "model")?;

    let (nonce, ciphertext) = encrypt(state, key)?;

    sqlx::query(
        "insert into user_api_keys (user_id, provider, key_enc, base_url, model)
         values ($1, $2, $3, $4, $5)
         on conflict (user_id) do update
            set provider = excluded.provider,
                key_enc = excluded.key_enc,
                base_url = excluded.base_url,
                model = excluded.model",
    )
    .bind(user.user_id)
    .bind(PROVIDER)
    // The nonce is prepended to the ciphertext, exactly as the OAuth state does: a
    // deterministic nonce under one key is the failure AES-GCM does not survive, so the nonce
    // is random per call and has to be stored beside the bytes it belongs to.
    .bind(prepend(&nonce, &ciphertext))
    .bind(base_url.as_deref())
    .bind(model.as_deref())
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("storing the key: {error}")))?;

    // The key is never logged — not even its length, which is a hint about which provider it
    // is. What is logged is that a key was set, which is what an operator needs.
    tracing::info!(
        user_id = %user.user_id,
        has_base_url = base_url.is_some(),
        has_model = model.is_some(),
        "BYOK key stored"
    );

    Ok(ByokDescription {
        set: true,
        provider: Some(PROVIDER.to_string()),
        base_url,
        model,
    })
}

/// Removes the account's key. Idempotent: a second call removes nothing and answers the same
/// thing, which is what a double-click on "remove" needs.
pub async fn clear(state: &AppState, user: &AuthUser) -> AppResult<ByokDescription> {
    sqlx::query("delete from user_api_keys where user_id = $1")
        .bind(user.user_id)
        .execute(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("removing the key: {error}")))?;

    tracing::info!(user_id = %user.user_id, "BYOK key removed");
    Ok(ByokDescription {
        set: false,
        provider: None,
        base_url: None,
        model: None,
    })
}

/// What the account has stored, **without the key**.
pub async fn describe(state: &AppState, user: &AuthUser) -> AppResult<ByokDescription> {
    let row: Option<(String, Option<String>, Option<String>)> =
        sqlx::query_as("select provider, base_url, model from user_api_keys where user_id = $1")
            .bind(user.user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the key row: {error}")))?;

    Ok(match row {
        Some((provider, base_url, model)) => ByokDescription {
            set: true,
            provider: Some(provider),
            base_url,
            model,
        },
        None => ByokDescription {
            set: false,
            provider: None,
            base_url: None,
            model: None,
        },
    })
}

/// The provider a single request should use: the user's own when they have one **and** their
/// plan grants BYOK, otherwise `None` (the caller uses the server's).
///
/// Called **once per request**, and the resulting handle is passed down: calling it again
/// mid-request would decrypt the key a second time and, worse, could return a different
/// answer if the row changed between two calls — a request half on one provider and half on
/// another is the kind of bug that shows up as a strange bill.
///
/// A plan that does not grant BYOK is `None` rather than an error: this is a *selection*, and
/// the caller's fallback (the server provider) is a legitimate outcome. An account whose plan
/// does not grant it but which has a stored key — a downgraded Power account — therefore
/// quietly uses the server's provider, which is why the key is left in place rather than
/// deleted.
pub async fn provider_for(
    state: &AppState,
    user: &AuthUser,
) -> AppResult<Option<Arc<dyn LlmProvider>>> {
    match crate::entitlements::require(state, user, Feature::Byok).await {
        Ok(()) => {}
        // The plan does not grant BYOK (or the device is read-only): use the server's
        // provider. Any *other* error is a real failure and propagates, because swallowing an
        // internal error here would silently bill us for a request the student meant to pay
        // for themselves.
        Err(AppError::Entitlement { .. }) | Err(AppError::Forbidden) => return Ok(None),
        Err(error) => return Err(error),
    }

    let row: Option<(Vec<u8>, Option<String>, Option<String>)> =
        sqlx::query_as("select key_enc, base_url, model from user_api_keys where user_id = $1")
            .bind(user.user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the key row: {error}")))?;

    let Some((key_enc, base_url, model)) = row else {
        return Ok(None);
    };

    let key = decrypt(state, &key_enc)?;

    // The user's endpoint and model, falling back to the server's. A user who supplied only a
    // key gets the configured endpoint, which is what makes a key for the default provider a
    // one-field form.
    let configured = state.config.llm.clone().unwrap_or(LlmConfig {
        base_url: String::new(),
        api_key: String::new(),
        model: String::new(),
    });
    let llm_config = LlmConfig {
        base_url: base_url.unwrap_or(configured.base_url),
        api_key: key,
        model: model.unwrap_or(configured.model),
    };

    if llm_config.base_url.trim().is_empty() {
        // A key with nowhere to send it is a configuration the student has to fix, and
        // `provider_error` (502) is the roadmap's code for "the upstream is not usable".
        return Err(AppError::Provider(
            "this account's key has no endpoint: set a base_url or ask an administrator to \
             configure the server's model provider"
                .to_string(),
        ));
    }

    let provider = OpenAiCompatLlm::new(&llm_config).map_err(|error| {
        AppError::Internal(anyhow::anyhow!("building a BYOK provider: {error}"))
    })?;

    tracing::debug!(user_id = %user.user_id, "using the account's own model provider");
    Ok(Some(Arc::new(provider)))
}

/// Encrypts `plaintext`, returning the nonce and the ciphertext separately.
fn encrypt(state: &AppState, plaintext: &str) -> AppResult<([u8; 12], Vec<u8>)> {
    let mut nonce_bytes = [0u8; 12];
    rand::Rng::fill_bytes(&mut rand::rng(), &mut nonce_bytes);
    let nonce = Nonce::from(nonce_bytes);

    let ciphertext = cipher(state)?
        .encrypt(&nonce, plaintext.as_bytes())
        .map_err(|_| AppError::Internal(anyhow::anyhow!("encrypting the key")))?;

    Ok((nonce_bytes, ciphertext))
}

/// Decrypts a stored `key_enc`.
///
/// A failure here is `Internal`, not a validation error: the ciphertext is ours, written by
/// [`set`], so a row that will not decrypt means the encryption key changed or the row was
/// damaged — neither of which the student can fix, and both of which need an operator.
fn decrypt(state: &AppState, stored: &[u8]) -> AppResult<String> {
    let invalid = || AppError::Internal(anyhow::anyhow!("a stored API key could not be decrypted"));

    if stored.len() < 13 {
        return Err(invalid());
    }
    let (nonce_bytes, ciphertext) = stored.split_at(12);
    let nonce: [u8; 12] = nonce_bytes.try_into().map_err(|_| invalid())?;

    let plaintext = cipher(state)?
        .decrypt(&Nonce::from(nonce), ciphertext)
        .map_err(|_| invalid())?;

    String::from_utf8(plaintext).map_err(|_| invalid())
}

/// The one cipher, from the module that already owns the key reader.
fn cipher(state: &AppState) -> AppResult<Aes256Gcm> {
    crate::auth::oauth_state::cipher(state)
}

/// `nonce || ciphertext`, the storage format the OAuth state also uses.
fn prepend(nonce: &[u8; 12], ciphertext: &[u8]) -> Vec<u8> {
    let mut combined = nonce.to_vec();
    combined.extend_from_slice(ciphertext);
    combined
}

/// A trimmed, non-empty optional field. An empty string is treated as "not supplied" rather
/// than stored: a `base_url` of `""` would produce a provider with nowhere to send anything,
/// which is a confusing way to say "use the default".
fn normalized(value: Option<&str>, field: &str) -> AppResult<Option<String>> {
    match value.map(str::trim) {
        None | Some("") => Ok(None),
        Some(trimmed) => {
            if trimmed.len() > 512 {
                return Err(AppError::Validation(format!(
                    "{field} must be at most 512 characters"
                )));
            }
            Ok(Some(trimmed.to_string()))
        }
    }
}

/// The plan that grants BYOK, for a message that names what to buy.
///
/// Derived from the matrix through `Feature::min_plan`, never written down: a plan rename
/// would otherwise leave a message naming a plan that does not exist.
pub fn required_plan(plans: &stvdy_core::config::PlansFile) -> Option<PlanId> {
    Feature::Byok.min_plan(plans)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_storage_format_round_trips_the_nonce_beside_the_ciphertext() {
        let nonce = [7u8; 12];
        let stored = prepend(&nonce, b"ciphertext");
        assert_eq!(stored.len(), 12 + 10);
        assert_eq!(&stored[..12], &nonce);
        assert_eq!(&stored[12..], b"ciphertext");
    }

    #[test]
    fn an_empty_optional_field_means_use_the_default() {
        assert_eq!(normalized(None, "base_url").unwrap(), None);
        assert_eq!(normalized(Some("   "), "base_url").unwrap(), None);
        assert_eq!(
            normalized(Some(" https://api.example/v1 "), "base_url").unwrap(),
            Some("https://api.example/v1".to_string())
        );
        assert!(normalized(Some(&"x".repeat(513)), "model").is_err());
    }

    #[test]
    fn byok_is_a_power_plan_feature() {
        // The gate is read from the matrix, so this asserts the shipped config's answer
        // rather than a second list: if Power stopped granting BYOK, the message would name
        // a different plan and this test would say so.
        let plans = crate::testing::plans();
        assert_eq!(required_plan(&plans), Some(PlanId::Power));
    }
}
