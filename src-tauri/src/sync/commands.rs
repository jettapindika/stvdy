//! The IPC surface: `Platform.sync` on the native side.
//!
//! Every command is a thin wrapper over [`SyncShell`]. The one thing worth knowing here is
//! `sync_configure`'s argument names: Tauri resolves a command's arguments by the
//! camelCase spelling of the Rust parameter (`baseUrl`), and the SPA sends snake_case
//! (`base_url`) — so this command is the one that declares `rename_all = "snake_case"`.
//! Without it the app would fail at runtime on the first configure with "missing required
//! key baseUrl", which no `cargo test` in this crate would see.

use std::sync::Arc;

use serde_json::Value;
use stvdy_sync::proto::Entity;
use tauri::{AppHandle, Emitter, State};
use uuid::Uuid;

use super::{
    SYNC_AUTH_EXPIRED_EVENT, SYNC_REVOKED_EVENT, SYNC_STATUS_EVENT, SyncNotice, SyncReport,
    SyncShell,
};

/// The shell's sync, as Tauri manages it.
///
/// `shell` is `None` when the cache could not be opened at startup (a read-only data
/// directory, a corrupt file). Every command then says so rather than pretending to have a
/// cache, and the app keeps working online through the REST routes.
pub struct ShellState {
    pub shell: Option<Arc<SyncShell>>,
}

impl ShellState {
    fn shell(&self) -> Result<&Arc<SyncShell>, String> {
        self.shell
            .as_ref()
            .ok_or_else(|| "the local cache could not be opened".to_string())
    }
}

/// Raises one of the shell's events. The single place that knows their names, so a
/// listener in the SPA and a call site here cannot drift.
pub fn emit(app: &AppHandle, notice: SyncNotice) {
    match notice {
        SyncNotice::Status(report) => {
            let _ = app.emit(SYNC_STATUS_EVENT, report);
        }
        // The SPA maps this to `{state: 'revoked'}` on the same channel as every other
        // status, so no second report is emitted: the shell has already wiped.
        SyncNotice::Revoked => {
            let _ = app.emit(SYNC_REVOKED_EVENT, ());
        }
        SyncNotice::AuthExpired => {
            let _ = app.emit(SYNC_AUTH_EXPIRED_EVENT, ());
        }
    }
}

/// Takes the current session's credentials: the API origin, the access token and this
/// device's id.
///
/// The SPA owns the session, so the shell is handed a token rather than reading one from
/// the keychain, and the token lives in memory only — it is never written to the cache. The
/// account the token names is what binds the cache; a different account wipes it first.
#[tauri::command(rename_all = "snake_case")]
pub fn sync_configure(
    state: State<'_, ShellState>,
    base_url: String,
    access_token: String,
    device_id: String,
) -> Result<(), String> {
    let device_id = Uuid::parse_str(&device_id).map_err(|error| format!("device id: {error}"))?;
    state
        .shell()?
        .configure(&base_url, &access_token, device_id)
        .map_err(|error| error.to_string())
}

/// The status, from stored facts: the last pass's outcome, the outbox's length, and the
/// timestamp the last successful pass wrote.
#[tauri::command]
pub fn sync_status(state: State<'_, ShellState>) -> Result<SyncReport, String> {
    state.shell()?.status().map_err(|error| error.to_string())
}

/// One pass: drain the outbox, pull every page, and report what happened.
#[tauri::command]
pub async fn sync_now(state: State<'_, ShellState>) -> Result<SyncReport, String> {
    let shell = state.shell()?.clone();
    shell.now().await.map_err(|error| error.to_string())
}

/// The cached row (`id` given), or the entity's rows as `[{id, row}, …]`.
#[tauri::command]
pub fn sync_read(
    state: State<'_, ShellState>,
    entity: Entity,
    id: Option<Uuid>,
) -> Result<Value, String> {
    state
        .shell()?
        .read(entity, id)
        .map_err(|error| error.to_string())
}

/// The local-first write: the row lands in the cache and its mutation in the outbox, and
/// the status is re-announced so a control cannot keep showing `synced` (A187).
#[tauri::command]
pub fn sync_write(
    state: State<'_, ShellState>,
    entity: Entity,
    id: Uuid,
    payload: Value,
) -> Result<(), String> {
    state
        .shell()?
        .write(entity, id, &payload)
        .map_err(|error| error.to_string())
}

/// The local-first tombstone.
#[tauri::command]
pub fn sync_tombstone(
    state: State<'_, ShellState>,
    entity: Entity,
    id: Uuid,
) -> Result<(), String> {
    state
        .shell()?
        .tombstone(entity, id)
        .map_err(|error| error.to_string())
}

/// "Reset local data and resync": the cache **and the outbox** go, then one pass rebuilds
/// from zero.
#[tauri::command]
pub async fn sync_reset(state: State<'_, ShellState>) -> Result<SyncReport, String> {
    let shell = state.shell()?.clone();
    shell.reset().await.map_err(|error| error.to_string())
}

/// The app's own state for `key` — a UI preference or a running focus timer's marker.
///
/// Deliberately not part of `Platform.sync`: this is device state, not synced data, and it
/// is not touched by `sync_reset` or by a revoked device. `None` covers both "never set"
/// and "deleted", because both are the same absence.
#[tauri::command]
pub fn local_state_get(
    state: State<'_, ShellState>,
    key: String,
) -> Result<Option<String>, String> {
    state
        .shell()?
        .local_state_get(&key)
        .map_err(|error| error.to_string())
}

/// Sets the app's own state for `key`, or clears it when `value` is absent.
///
/// `value: Option<String>` is what makes a missing argument mean "delete the row" rather
/// than "store the text `null`": the SPA's `delete(key)` sends `value: null`, and a stored
/// `"null"` would read back as a tombstone instead of as absent.
#[tauri::command]
pub fn local_state_set(
    state: State<'_, ShellState>,
    key: String,
    value: Option<String>,
) -> Result<(), String> {
    state
        .shell()?
        .local_state_set(&key, value.as_deref())
        .map_err(|error| error.to_string())
}
