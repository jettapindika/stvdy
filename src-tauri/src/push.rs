//! Push receive, behind one seam: the provider string, the token check, and the
//! single call into the plugin.
//!
//! **The seam is the point (A310).** The plugin choice is a pinned community crate
//! (`tauri-plugin-notifications`, Choochmeque 0.5.0-rc.14) with no official
//! alternative — the official `tauri-plugin-notification` provably cannot receive
//! remote push, because its iOS delegate skips `UNPushNotificationTrigger` and its
//! Android manifest registers no `com.google.firebase.MESSAGING_EVENT` service. If
//! that crate regresses before submission the replacement is a custom plugin, and
//! this module is the whole surface that changes: [`provider_for`],
//! [`validate_token`] and [`register`] are the only things outside this file that
//! know a plugin exists at all.
//!
//! **The pure half is deliberately not `cfg`-gated.** `provider_for` and
//! `validate_token` are the parts that can be wrong in a way no device would ever
//! reveal — a provider string the server's `PUSH_PROVIDERS` does not accept sits in
//! `push_tokens` and never delivers, and an empty token is a `PUT` that can only be
//! refused. Both are asserted by `tests/push.rs` **on macOS**, which is why the
//! plugin dependency is target-gated to iOS/Android in `Cargo.toml` rather than
//! pulled in for every platform: a seam that can only be compiled on a phone is a
//! seam nobody can test.
//!
//! This module holds no secret. `google-services.json` and the APNs key are
//! supplied at build time and are not committed (A328); what the SPA receives here
//! is a token the OS minted for this install.

use serde::Serialize;

/// The provider strings the server accepts, mirroring `PUSH_PROVIDERS` in
/// `crates/server/src/devices.rs`. A third string is a typo that the server rejects
/// with a 422 — so the two lists are cross-checked by `tests/push.rs` rather than
/// trusted to stay in step.
pub const PROVIDERS: [&str; 2] = ["apns", "fcm"];

/// What the SPA `PUT`s to `/devices/{id}/push-token` (A329).
///
/// Both halves are here because the route wants both: the provider is derived from
/// the platform rather than read from the token, so a client that guessed it wrong
/// would write a row that never delivers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct PushToken {
    /// `"apns"` or `"fcm"`.
    pub provider: &'static str,
    pub token: String,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum PushError {
    #[error("a push token cannot be empty")]
    EmptyToken,
    /// The platform has no push provider in the server's list. On macOS/Windows/Linux
    /// this is the whole story: push receive is a phone feature here (A310's matrix),
    /// and the SPA's `Platform.push` is only wired on `ios`/`android`.
    #[error("this platform has no push provider")]
    UnknownPlatform,
    #[error("push registration failed: {0}")]
    Registration(String),
}

/// The provider string for a platform kind.
///
/// `kind` is the same word the SPA's `platform.kind()` returns and the same one
/// `std::env::consts::OS` carries, so there is one vocabulary for "which platform is
/// this" rather than a second mapping that could disagree with it.
///
/// Returns `None` for every platform that is not a phone, deliberately: the caller
/// that registers a token must not be able to invent a provider for a desktop, and
/// `None` is what makes the server's two-string list a closed set here too.
pub fn provider_for(kind: &str) -> Option<&'static str> {
    match kind {
        "ios" => Some("apns"),
        "android" => Some("fcm"),
        _ => None,
    }
}

/// The provider for the platform this binary was compiled for.
pub fn provider_here() -> Option<&'static str> {
    provider_for(std::env::consts::OS)
}

/// Checks a token before it is handed to the network.
///
/// The server's own rule (`devices.rs`: `token.trim().is_empty()`) is the same one,
/// applied one hop earlier — a whitespace-only token would be a round trip that can
/// only be refused, and the device would then have no token registered at all while
/// the log said "registered". The token is returned trimmed, so what the SPA sends
/// is what was validated.
///
/// Borrows rather than allocating: the caller already owns the `String` the OS gave
/// us, and copying it here would be a copy with no reader.
pub fn validate_token(token: &str) -> Result<&str, PushError> {
    let trimmed = token.trim();
    if trimmed.is_empty() {
        return Err(PushError::EmptyToken);
    }
    Ok(trimmed)
}

/// Asks the plugin for this device's push token, and returns it with its provider.
///
/// **`cfg`-gated to the platforms that have a push provider.** This is the only
/// function in the shell that names the plugin, and it exists on iOS and Android
/// only, because that is exactly where the plugin dependency is declared — so a
/// desktop build cannot call it and cannot be broken by it.
///
/// **Async because the plugin's mobile half is.** On iOS and Android both calls
/// below cross into the native plugin over `run_mobile_plugin_async`, which is a
/// round trip through the platform's own permission and APNs/FCM machinery; the
/// desktop half of the same crate is synchronous, which is why the shape is not
/// obvious from the crate's documentation page. An `await` that returned a `Future`
/// unawaited would be a token registration that silently never happened.
#[cfg(any(target_os = "ios", target_os = "android"))]
pub async fn register(app: &tauri::AppHandle) -> Result<PushToken, PushError> {
    use tauri_plugin_notifications::NotificationsExt;

    let provider = provider_here().ok_or(PushError::UnknownPlatform)?;
    let notifications = app.notifications();

    // The prompt is raised from the one call that needs the answer, rather than from
    // the SPA: a permission requested at a moment the user did not ask for a
    // notification is the prompt that gets denied. A failure is not fatal — the
    // registration below reports the real reason.
    match notifications.request_permission().await {
        Ok(state) => tracing::info!(?state, "push permission requested"),
        Err(error) => tracing::warn!(%error, "the push permission prompt could not be shown"),
    }

    let token = notifications
        .register_for_push_notifications()
        .await
        .map_err(|error| PushError::Registration(error.to_string()))?;
    let token = validate_token(&token)?;

    // The token itself is a device credential and is never logged; the provider and
    // the length are what make "registration happened" searchable.
    tracing::info!(provider, token_len = token.len(), "push token registered");

    Ok(PushToken {
        provider,
        token: token.to_string(),
    })
}

/// The token this device should register, for the SPA to `PUT` (A329).
///
/// On a phone this asks the OS; elsewhere it says so. The command is registered on
/// every platform so the SPA's `Platform.push` has one shape, and the desktop answer
/// is an error rather than a fabricated provider — `provider_for` returns `None` for
/// a desktop on purpose, and a `"apns"` token minted for a Mac would be a row the
/// server accepts and the APNs gateway cannot deliver.
#[tauri::command]
pub async fn push_token(app: tauri::AppHandle) -> Result<PushToken, String> {
    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        register(&app).await.map_err(|error| error.to_string())
    }
    #[cfg(not(any(target_os = "ios", target_os = "android")))]
    {
        let _ = &app;
        Err(PushError::UnknownPlatform.to_string())
    }
}
