//! The Android share intake entry point (A311, A313, A314).
//!
//! **The native half does the reading and this half does the filing.** Android
//! delivers a shared file as a `content://` URI whose read grant is transient
//! (A314: `ACTION_GET_CONTENT` grants die when the receiving activity finishes, and
//! nothing in the dialog plugin calls `takePersistableUriPermission`). The Kotlin
//! side is therefore the only place that *can* read it — a `content://` URI is
//! resolved by the content resolver, not by `std::fs` — and it copies the bytes into
//! this plugin's own inbox under the app's data directory before returning. What
//! arrives here is an ordinary file path in app storage, which is what the upload
//! queue can hold.
//!
//! **No webview.** A share can arrive while the app has no window at all, so nothing
//! in this module touches one: it writes a file and a sidecar, and returns. The SPA
//! reads the result later, on its own schedule, through the shell's share surface.
//!
//! The inbox is one directory with one convention, so the shell's reader (Task 5's
//! `src-tauri/src/share.rs`) and this writer cannot disagree about the layout:
//! `<inbox>/<id>.<ext>` holds the bytes and `<inbox>/<id>.json` holds the metadata.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};
use tauri::{AppHandle, Manager, Runtime};

/// The inbox directory, inside the app's data directory. Named once, here.
pub const INBOX_DIR: &str = "share-inbox";

#[derive(Debug, thiserror::Error)]
pub enum IntakeError {
    #[error("there is no app data directory to file a shared file into")]
    NoDataDir,
    #[error("the shared file could not be read: {0}")]
    Read(String),
    #[error("the shared file could not be written: {0}")]
    Write(String),
}

/// One file that arrived from the share sheet or an "open with" intent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFile {
    /// Opaque, unique per intake. What the SPA quotes back to clear the row.
    pub id: String,
    /// The name the *sending* app showed, which is what a user recognises. Not
    /// trusted as a path: it is recorded as metadata and never joined onto the
    /// inbox path.
    pub name: String,
    /// Where the bytes now live, inside app storage. This is what the upload queue
    /// holds.
    pub path: PathBuf,
    /// The intent's MIME type, when it carried one.
    pub mime: Option<String>,
}

/// The inbox inside an app data directory.
pub fn inbox_dir(data_dir: &Path) -> PathBuf {
    data_dir.join(INBOX_DIR)
}

/// Files a shared file into the inbox, durably.
///
/// `source` is a path the Kotlin half already copied into app storage (see the
/// module docs for why it cannot be a `content://` URI here). The bytes are *moved*
/// rather than copied when the source is already in app storage, and the metadata
/// sidecar is written only after the bytes land — a sidecar without bytes is a row
/// the SPA would show and then fail to upload, which is worse than no row.
pub fn accept<R: Runtime>(
    app: &AppHandle<R>,
    source: &Path,
    name: &str,
    mime: Option<&str>,
) -> Result<SharedFile, IntakeError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| IntakeError::NoDataDir)?;
    let inbox = inbox_dir(&data_dir);
    std::fs::create_dir_all(&inbox).map_err(|error| IntakeError::Write(error.to_string()))?;

    // The id is a fresh v4 UUID rather than a content hash: two shares of the same
    // file are two intents, and collapsing them would silently drop the second one.
    let id = uuid::Uuid::new_v4().to_string();
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bin");
    let destination = inbox.join(format!("{id}.{extension}"));

    std::fs::rename(source, &destination)
        .or_else(|_| std::fs::copy(source, &destination).map(|_| ()))
        .map_err(|error| IntakeError::Read(error.to_string()))?;

    let file = SharedFile {
        id: id.clone(),
        name: name.to_string(),
        path: destination,
        mime: mime.map(str::to_string),
    };

    let sidecar = inbox.join(format!("{id}.json"));
    let encoded = serde_json::to_vec(&file).map_err(|error| IntakeError::Write(error.to_string()))?;
    std::fs::write(&sidecar, encoded).map_err(|error| IntakeError::Write(error.to_string()))?;

    tracing::info!(
        id,
        name,
        mime,
        // The bytes' own length, so a zero-byte share is visible in the log rather
        // than only as a failed upload later.
        bytes = file.path.metadata().map(|meta| meta.len()).unwrap_or(0),
        "shared file filed into the inbox"
    );
    Ok(file)
}

/// What the inbox currently holds, oldest id first.
///
/// A sidecar with no bytes is skipped: the pair is the unit, and a half-written
/// intake is one the SPA cannot upload.
pub fn pending<R: Runtime>(app: &AppHandle<R>) -> Result<Vec<SharedFile>, IntakeError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| IntakeError::NoDataDir)?;
    let inbox = inbox_dir(&data_dir);
    let entries = match std::fs::read_dir(&inbox) {
        Ok(entries) => entries,
        // No inbox is not an error: nothing has ever been shared.
        Err(_) => return Ok(Vec::new()),
    };

    let mut files = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|value| value.to_str()) != Some("json") {
            continue;
        }
        let Ok(encoded) = std::fs::read(&path) else {
            continue;
        };
        let Ok(file) = serde_json::from_slice::<SharedFile>(&encoded) else {
            tracing::warn!(path = %path.display(), "an inbox sidecar is not readable; skipped");
            continue;
        };
        if !file.path.is_file() {
            tracing::warn!(id = file.id, "an inbox sidecar has no bytes; skipped");
            continue;
        }
        files.push(file);
    }
    files.sort_by(|a, b| a.id.cmp(&b.id));
    Ok(files)
}

/// Removes an intake once the SPA has taken responsibility for it.
///
/// Both halves go: leaving the sidecar would resurrect the row on the next read, and
/// leaving the bytes would grow the inbox for ever.
pub fn clear<R: Runtime>(app: &AppHandle<R>, id: &str) -> Result<(), IntakeError> {
    let data_dir = app
        .path()
        .app_data_dir()
        .map_err(|_| IntakeError::NoDataDir)?;
    let inbox = inbox_dir(&data_dir);

    for file in pending(app)? {
        if file.id != id {
            continue;
        }
        std::fs::remove_file(&file.path).map_err(|error| IntakeError::Write(error.to_string()))?;
    }
    let sidecar = inbox.join(format!("{id}.json"));
    match std::fs::remove_file(&sidecar) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(IntakeError::Write(error.to_string())),
    }
}
