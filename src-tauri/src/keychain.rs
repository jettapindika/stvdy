//! Secure token storage.
//!
//! Only the refresh token is stored here; the access token lives in memory for
//! the process lifetime. On Windows the blob is limited to 2560 bytes
//! (`CRED_MAX_CREDENTIAL_BLOB_SIZE`), which is another reason to persist only
//! the refresh token rather than a serialized session.
//!
//! `keyring-core` keeps a process-wide default store; each platform installs
//! its own before the first `Entry` is created. Verified against keyring-core
//! 1.0.0 (`set_default_store(Arc<CredentialStore>)`,
//! `Entry::{new,set_password,get_password,delete_credential}`,
//! `Error::{NoEntry,NoDefaultStore}`) and apple-native-keyring-store 1.0.2
//! (`Store::new() -> Result<Arc<Self>>`, features `keychain`/`protected`).

use std::sync::Arc;

use keyring_core::{CredentialStore, Entry, Error as KeyringError};

pub const SERVICE: &str = "app.stvdy";

#[derive(Debug, thiserror::Error)]
pub enum SecretError {
    #[error("no secure store is available on this platform")]
    NoStore,
    #[error("secure storage failed: {0}")]
    Backend(String),
}

/// Whether this OS has an OS credential store we can use.
///
/// Pure data rather than `cfg`, so the answer is testable on every platform. The
/// `cfg`-gated wiring in `init()` can only be compiled on the platforms it
/// targets, which means the "no store here" branch was previously exercised
/// nowhere: every job that builds `src-tauri` runs macOS or Windows, and the
/// ubuntu jobs use `--workspace`, which excludes it. A later refactor could have
/// broken the `NoStore` arm without failing anything.
pub fn has_secure_store(os: &str) -> bool {
    matches!(os, "macos" | "ios" | "windows" | "android")
}

/// Installs the platform store exactly once, before any Entry is created.
pub fn init() -> Result<(), SecretError> {
    // Keeps the pure mapping above and the cfg-gated wiring below from drifting:
    // if they disagree, this fires on the first call rather than in production.
    debug_assert_eq!(
        has_secure_store(std::env::consts::OS),
        cfg!(any(
            target_os = "macos",
            target_os = "ios",
            target_os = "windows",
            target_os = "android"
        )),
        "has_secure_store and the cfg gates disagree for {}",
        std::env::consts::OS
    );

    #[cfg(target_os = "macos")]
    {
        // The legacy keychain works for unsigned dev builds; the protected data
        // store requires a provisioning profile.
        let store: Arc<CredentialStore> = apple_native_keyring_store::keychain::Store::new()
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        keyring_core::set_default_store(store);
        Ok(())
    }
    #[cfg(target_os = "ios")]
    {
        // iOS has only the protected data store.
        let store: Arc<CredentialStore> = apple_native_keyring_store::protected::Store::new()
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        keyring_core::set_default_store(store);
        Ok(())
    }
    #[cfg(target_os = "windows")]
    {
        let store: Arc<CredentialStore> = windows_native_keyring_store::Store::new()
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        keyring_core::set_default_store(store);
        Ok(())
    }
    #[cfg(target_os = "android")]
    {
        // No shim needed: android-activity (via tao, which Tauri uses) calls
        // ndk_context::initialize_android_context before main, and this store's
        // README lists Tauri Mobile as a framework that already provides that.
        // Calling it ourselves would be a SECOND call, and ndk-context asserts
        // the call happens exactly once (`assert!(previous.is_none())`), so it
        // would panic at startup. If a device prototype ever shows Store::new()
        // returning Err here, that is the trigger to revisit (ASSUMPTIONS A30).
        let store: Arc<CredentialStore> = android_native_keyring_store::Store::new()
            .map_err(|e| SecretError::Backend(e.to_string()))?;
        keyring_core::set_default_store(store);
        Ok(())
    }
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    {
        Err(SecretError::NoStore)
    }
}

fn entry(key: &str) -> Result<Entry, SecretError> {
    Entry::new(SERVICE, key).map_err(|e| match e {
        KeyringError::NoDefaultStore => SecretError::NoStore,
        other => SecretError::Backend(other.to_string()),
    })
}

pub fn set(key: &str, value: &str) -> Result<(), SecretError> {
    entry(key)?
        .set_password(value)
        .map_err(|e| SecretError::Backend(e.to_string()))
}

pub fn get(key: &str) -> Result<Option<String>, SecretError> {
    match entry(key)?.get_password() {
        Ok(value) => Ok(Some(value)),
        Err(KeyringError::NoEntry) => Ok(None),
        Err(other) => Err(SecretError::Backend(other.to_string())),
    }
}

pub fn delete(key: &str) -> Result<(), SecretError> {
    match entry(key)?.delete_credential() {
        Ok(()) | Err(KeyringError::NoEntry) => Ok(()),
        Err(other) => Err(SecretError::Backend(other.to_string())),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The Phase 0/1 native prototype: prove the keychain round-trips on this
    /// machine before any auth code depends on it. On macOS the first run may
    /// prompt for keychain access; that prompt is the expected result to record
    /// in docs/ARCHITECTURE.md.
    ///
    /// Gated to platforms that have a store: `init()` returns `NoStore` on Linux,
    /// so an ungated test would FAIL there (not skip), breaking `just check` for
    /// any Linux contributor and for any future ubuntu shell job.
    #[cfg(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    ))]
    #[test]
    fn keychain_round_trips_a_refresh_token() {
        init().expect("a secure store must be available on this platform");

        let key = "phase0-probe";
        let token = "refresh-token-value";

        // Start clean so a previous run cannot make this pass vacuously.
        delete(key).expect("delete must succeed even when absent");
        assert_eq!(get(key).expect("get must not error"), None);

        set(key, token).expect("set must store the secret");
        assert_eq!(
            get(key).expect("get must read it back").as_deref(),
            Some(token)
        );

        set(key, "rotated").expect("set must overwrite");
        assert_eq!(
            get(key).expect("get must read the new value").as_deref(),
            Some("rotated")
        );

        delete(key).expect("delete must remove it");
        assert_eq!(get(key).expect("get after delete must be None"), None);
    }

    /// The complement: where there is no OS store, `init()` must say so rather
    /// than appearing to succeed. This exercises the `NoStore` path instead of
    /// leaving it untested.
    #[cfg(not(any(
        target_os = "macos",
        target_os = "ios",
        target_os = "windows",
        target_os = "android"
    )))]
    #[test]
    fn init_reports_no_store_on_unsupported_platforms() {
        assert!(matches!(init(), Err(SecretError::NoStore)));
    }

    /// The platform matrix, checked everywhere rather than only on the platforms
    /// that happen to have a store. This is what makes the `NoStore` branch
    /// covered: on Linux it asserts the mapping says "no store"; on macOS it
    /// asserts the mapping agrees with the cfg that actually compiled.
    #[test]
    fn secure_store_availability_matches_the_platform_matrix() {
        for os in ["macos", "ios", "windows", "android"] {
            assert!(has_secure_store(os), "{os} has an OS credential store");
        }
        for os in ["linux", "freebsd", "openbsd", "netbsd", "dragonfly", ""] {
            assert!(
                !has_secure_store(os),
                "{os} has no store, so init() must return NoStore"
            );
        }
    }
}
