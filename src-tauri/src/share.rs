//! The share inbox: the App Group the iOS Share Extension writes into, and the reader
//! the app imports from when it returns to the foreground (A312, A313, A314).
//!
//! **Why a shared container and not a URL.** A Share Extension cannot launch its host
//! app — `NSExtensionContext.open(_:)` is supported only by the Today and iMessage
//! extension points — so the extension cannot hand the file over by opening a
//! `stvdy://` URL. The two processes' only shared surface is an App Group container, so
//! the extension copies the bytes there and returns, and this module reads them on the
//! next foreground.
//!
//! **The identifier is one string.** The extension, both entitlements blocks and this
//! module name the same container; a second spelling on either side is a silent "the
//! file never arrives", because a container that does not exist is a directory that
//! reads empty and nothing fails. `tests/share_group.rs` asserts the literal appears
//! once here and once in the extension, and that no other container name exists in the
//! shell's sources.
//!
//! **The inbox format is one convention, shared with the mobile plugin.** Android
//! delivers a shared file as a `content://` URI instead, which the plugin's Kotlin half
//! copies into app storage and its Rust half files into an inbox of the same shape —
//! `<id>.<ext>` for the bytes and `<id>.json` for the metadata — so this reader serves
//! both platforms, and `tests/share_group.rs` pins the sidecar's JSON against the
//! literal the plugin serializes.
//!
//! **The reader is pure over a directory.** Only [`inbox`] knows which directory that
//! is, and it is the one function `cfg`-gated to the phone platforms; everything else
//! compiles and is tested on the machine the tests run on.

use std::path::{Path, PathBuf};

use serde::{Deserialize, Serialize};

/// The App Group the Share Extension and the app share. Named once.
pub const APP_GROUP: &str = "group.app.stvdy";

/// The inbox inside the App Group container. The extension writes here; the app reads
/// here and deletes what it has taken.
pub const GROUP_INBOX: &str = "Inbox";

/// The Android inbox, inside the app's own data directory. **`ShareIntake.kt`'s
/// `INBOX` is the same string**, asserted by `tests/share_android.rs`: a second
/// spelling on either side is an inbox that reads empty and nothing fails.
pub const ANDROID_INBOX: &str = "share-inbox";

/// Where iOS keeps App Group containers inside the app's own sandbox. The container is
/// created by the OS from the entitlement, so this path exists exactly when the
/// entitlement is present — a build without it reads an empty directory rather than
/// failing, which is why the entitlement is asserted in `tests/share_group.rs`.
const GROUP_CONTAINERS: &str = "Library/Group Containers";

/// The event the shell emits when the inbox holds something, so the SPA does not have
/// to poll. Payload: `Vec<SharedFile>`.
pub const SHARE_PENDING_EVENT: &str = "stvdy://share-pending";

/// The App Group container inside an app's sandbox.
pub fn group_container(home: &Path) -> PathBuf {
    home.join(GROUP_CONTAINERS).join(APP_GROUP)
}

/// The inbox inside an app's sandbox.
pub fn inbox_dir(home: &Path) -> PathBuf {
    group_container(home).join(GROUP_INBOX)
}

#[derive(Debug, thiserror::Error)]
pub enum ShareError {
    #[error("there is no share inbox on this platform")]
    NoInbox,
    #[error("the inbox could not be read: {0}")]
    Read(String),
    #[error("the inbox could not be written: {0}")]
    Write(String),
    /// A `content://` URI is resolved by Android's content resolver, not by `std::fs`,
    /// and its read grant dies with the receiving activity (A314). Only the Kotlin
    /// half can read one, and it does so before this is ever called.
    #[error("a content:// URI cannot be read here; the native intake must copy it first")]
    ContentUri,
}

/// One file that arrived from the share sheet or an "open with" intent.
///
/// The field names are the sidecar's JSON, which the mobile plugin's `intake` module
/// writes for Android and the Share Extension writes for iOS — one format, three
/// authors, pinned by a literal fixture in both crates' tests.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SharedFile {
    /// Opaque, unique per intake. What the SPA quotes back to clear the row.
    pub id: String,
    /// The name the *sending* app showed, which is what a user recognises. Never
    /// joined onto a path: it is metadata, and the bytes' own name is the id.
    pub name: String,
    /// Where the bytes are, inside the shared container.
    pub path: PathBuf,
    /// The intent's MIME type, when it carried one.
    pub mime: Option<String>,
}

/// What the inbox currently holds, oldest id first.
///
/// A sidecar with no bytes is skipped: the pair is the unit, and a half-written intake
/// is one the SPA cannot upload.
pub fn pending_in(dir: &Path) -> Result<Vec<SharedFile>, ShareError> {
    let entries = match std::fs::read_dir(dir) {
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
            tracing::warn!(sidecar = %path.display(), "an inbox sidecar is not readable; skipped");
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
pub fn clear_in(dir: &Path, id: &str) -> Result<(), ShareError> {
    // The id is the SPA's, so it is matched against the inbox rather than joined onto
    // a path — an id of `../../x` would otherwise delete outside the container.
    for file in pending_in(dir)? {
        if file.id != id {
            continue;
        }
        std::fs::remove_file(&file.path).map_err(|error| ShareError::Write(error.to_string()))?;
    }
    let sidecar = dir.join(format!("{id}.json"));
    match std::fs::remove_file(&sidecar) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(ShareError::Write(error.to_string())),
    }
}

/// Copies a shared file into app storage, durably, before the read grant expires.
///
/// `source` is what the native side handed us: a `file://` path, or a bare path. A
/// `content://` URI is refused with a reason rather than attempted — `std::fs` cannot
/// resolve one, and a copy that silently produced a zero-byte file would be an upload
/// that fails much later, on a screen nobody can debug from. The name is a fresh v4
/// UUID and the extension is the source's own, so a name from another app is never
/// joined onto a path.
pub fn copy_into_storage(source: &str, destination_dir: &Path) -> Result<PathBuf, ShareError> {
    if source.starts_with("content://") {
        return Err(ShareError::ContentUri);
    }
    let source = Path::new(source.strip_prefix("file://").unwrap_or(source));
    if !source.is_file() {
        return Err(ShareError::Read(format!(
            "there is no file at {}",
            source.display()
        )));
    }

    std::fs::create_dir_all(destination_dir)
        .map_err(|error| ShareError::Write(error.to_string()))?;
    let extension = source
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("bin");
    let destination = destination_dir.join(format!("{}.{extension}", uuid::Uuid::new_v4()));

    std::fs::copy(source, &destination).map_err(|error| ShareError::Read(error.to_string()))?;
    Ok(destination)
}

/// The inbox this process should read.
///
/// iOS reads the App Group container, whose path the OS derives from the app's own
/// sandbox (`HOME`) plus the entitlement; Android reads the directory the mobile
/// plugin's Kotlin half copies into, named once in that crate; a desktop has no share
/// sheet, and saying so is better than reading a directory that will always be empty.
#[cfg(target_os = "ios")]
pub fn inbox<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) -> Result<PathBuf, ShareError> {
    let home = std::env::var_os("HOME").ok_or(ShareError::NoInbox)?;
    Ok(inbox_dir(Path::new(&home)))
}

#[cfg(target_os = "android")]
pub fn inbox<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Result<PathBuf, ShareError> {
    use tauri::Manager;
    let data_dir = app.path().app_data_dir().map_err(|_| ShareError::NoInbox)?;
    Ok(data_dir.join(ANDROID_INBOX))
}

#[cfg(not(any(target_os = "ios", target_os = "android")))]
pub fn inbox<R: tauri::Runtime>(_app: &tauri::AppHandle<R>) -> Result<PathBuf, ShareError> {
    Err(ShareError::NoInbox)
}

/// The inbox's contents, or nothing on a platform that has no inbox.
///
/// An absent inbox is an empty list rather than an error: the SPA asks on every boot,
/// and a desktop answer of "there is no share sheet here" would be a red toast on a
/// perfectly healthy Mac.
pub fn pending<R: tauri::Runtime>(app: &tauri::AppHandle<R>) -> Vec<SharedFile> {
    match inbox(app) {
        Ok(dir) => pending_in(&dir).unwrap_or_else(|error| {
            tracing::warn!(%error, "the share inbox could not be read");
            Vec::new()
        }),
        Err(_) => Vec::new(),
    }
}

/// Reads the inbox and tells the SPA, if there is anything to tell it.
///
/// Called from `setup` and on every resume. A cold start may have no window listening
/// yet, which is why the SPA also has the `share_pending` command: the event is the
/// fast path, the command is the one that cannot be missed.
pub fn announce<R: tauri::Runtime>(app: &tauri::AppHandle<R>) {
    let files = pending(app);
    if files.is_empty() {
        return;
    }
    use tauri::Emitter;
    tracing::info!(count = files.len(), "shared files are waiting in the inbox");
    let _ = app.emit(SHARE_PENDING_EVENT, files);
}

/// The pending imports, for the SPA.
#[tauri::command]
pub fn share_pending<R: tauri::Runtime>(app: tauri::AppHandle<R>) -> Vec<SharedFile> {
    pending(&app)
}

/// Removes an intake once the SPA has taken it, so the row does not come back.
#[tauri::command]
pub fn share_clear<R: tauri::Runtime>(app: tauri::AppHandle<R>, id: String) -> Result<(), String> {
    let dir = inbox(&app).map_err(|error| error.to_string())?;
    clear_in(&dir, &id).map_err(|error| error.to_string())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn scratch(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!("stvdy-share-{}-{name}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).expect("a scratch directory");
        dir
    }

    #[test]
    fn the_container_path_is_the_entitlement_inside_the_sandbox() {
        let home = Path::new("/var/mobile/Containers/Data/Application/ABC");
        assert_eq!(
            inbox_dir(home),
            home.join("Library/Group Containers")
                .join(APP_GROUP)
                .join("Inbox")
        );
    }

    #[test]
    fn a_sidecar_without_bytes_is_not_a_pending_file() {
        let dir = scratch("sidecar-only");
        let file = SharedFile {
            id: "a".to_string(),
            name: "notes.pdf".to_string(),
            path: dir.join("a.pdf"),
            mime: Some("application/pdf".to_string()),
        };
        std::fs::write(
            dir.join("a.json"),
            serde_json::to_vec(&file).expect("a sidecar"),
        )
        .expect("written");

        assert!(pending_in(&dir).expect("readable").is_empty());
    }

    #[test]
    fn a_file_with_bytes_is_pending_and_clears_both_halves() {
        let dir = scratch("complete");
        let bytes = dir.join("b.pdf");
        std::fs::write(&bytes, b"%PDF-1.4").expect("bytes");
        let file = SharedFile {
            id: "b".to_string(),
            name: "notes.pdf".to_string(),
            path: bytes.clone(),
            mime: Some("application/pdf".to_string()),
        };
        std::fs::write(
            dir.join("b.json"),
            serde_json::to_vec(&file).expect("a sidecar"),
        )
        .expect("written");

        assert_eq!(pending_in(&dir).expect("readable"), vec![file]);
        clear_in(&dir, "b").expect("cleared");
        assert!(pending_in(&dir).expect("readable").is_empty());
        assert!(!bytes.exists(), "the bytes go with the sidecar");
    }

    #[test]
    fn a_shared_file_is_copied_under_a_name_of_our_own() {
        let source = scratch("source");
        let from = source.join("someone-elses name.pdf");
        std::fs::write(&from, b"%PDF-1.4 body").expect("the shared file");
        let destination = scratch("destination");

        let copied =
            copy_into_storage(from.to_str().expect("a utf-8 path"), &destination).expect("copied");

        assert_eq!(std::fs::read(&copied).expect("readable"), b"%PDF-1.4 body");
        assert_eq!(copied.extension().and_then(|e| e.to_str()), Some("pdf"));
        assert_eq!(copied.parent(), Some(destination.as_path()));
        assert!(
            !copied
                .file_name()
                .and_then(|n| n.to_str())
                .unwrap_or_default()
                .contains("someone"),
            "the sender's name is metadata, never a path component"
        );
    }

    #[test]
    fn a_content_uri_is_refused_rather_than_copied_as_nothing() {
        let destination = scratch("content-uri");
        let error = copy_into_storage("content://media/external/file/42", &destination)
            .expect_err("a content:// URI is not readable by std::fs");
        assert!(matches!(error, ShareError::ContentUri));
    }

    /// The literal the mobile plugin's `intake::SharedFile` serializes to. That crate's
    /// own `tests/intake.rs` asserts it produces exactly this string, so the two crates
    /// cannot drift apart without one of the two tests going red.
    #[test]
    fn the_shell_reads_the_sidecar_the_plugin_writes() {
        let sidecar = r#"{"id":"11111111-1111-4111-8111-111111111111","name":"notes.pdf","path":"/tmp/inbox/x.pdf","mime":"application/pdf"}"#;
        let file: SharedFile = serde_json::from_str(sidecar).expect("the plugin's sidecar shape");
        assert_eq!(file.id, "11111111-1111-4111-8111-111111111111");
        assert_eq!(file.name, "notes.pdf");
        assert_eq!(file.path, Path::new("/tmp/inbox/x.pdf"));
        assert_eq!(file.mime.as_deref(), Some("application/pdf"));
    }
}
