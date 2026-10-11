use crate::keychain;

/// Round-trips through the IPC boundary so the frontend can prove the shell is
/// alive (used by the Phase 1 native prototype checklist).
#[tauri::command]
pub fn health_ping() -> String {
    format!("stvdy-shell {}", env!("CARGO_PKG_VERSION"))
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
