use tauri_plugin_deep_link::DeepLinkExt;

use crate::{deeplink, keychain};

/// Round-trips through the IPC boundary so the frontend can prove the shell is
/// alive (used by the Phase 1 native prototype checklist).
#[tauri::command]
pub fn health_ping() -> String {
    format!("stvdy-shell {}", env!("CARGO_PKG_VERSION"))
}

/// The URLs the app was launched with, classified into the same
/// `{action, payload}` shape the running-instance event delivers.
///
/// A cold start on macOS/iOS delivers the launch URL through the plugin's
/// `current` slot rather than (or before) the event, so the SPA pulls this once
/// on boot; a running instance uses the emitted event instead.
#[tauri::command]
pub fn deep_link_current(app: tauri::AppHandle) -> Result<Vec<deeplink::DeepLinkIntent>, String> {
    let urls = app
        .deep_link()
        .get_current()
        .map_err(|error| error.to_string())?;
    Ok(urls
        .unwrap_or_default()
        .iter()
        .map(|url| deeplink::classify(url.as_str()))
        .collect())
}

/// Stores the refresh token in the OS keychain/keystore.
#[tauri::command]
pub fn secret_set(key: String, value: String) -> Result<(), String> {
    keychain::set(&key, &value).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn secret_get(key: String) -> Result<Option<String>, String> {
    keychain::get(&key).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn secret_delete(key: String) -> Result<(), String> {
    keychain::delete(&key).map_err(|e| e.to_string())
}
