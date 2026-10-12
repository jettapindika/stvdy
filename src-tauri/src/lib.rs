pub mod cache;
pub mod commands;
pub mod deeplink;
pub mod keychain;
pub mod sync;

use tauri::Manager;

/// The SQLite cache's file name inside the app's data directory. The file holds one
/// account's synced rows and its outbox, never a token (the keychain is the only place a
/// token may live).
const CACHE_FILE: &str = "sync-cache.sqlite3";

/// Emitted to the frontend with a `Vec<DeepLinkIntent>` payload whenever the OS
/// hands the shell one or more `stvdy://` URLs while the app is running.
pub const DEEP_LINK_EVENT: &str = "stvdy://deep-link-intent";

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_from_default_env()
                .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new("info")),
        )
        .init();

    // A failure here is not fatal: the app still runs, it just cannot persist a
    // session. Warn rather than abort so a keychain prompt or a missing store
    // does not block the UI.
    if let Err(error) = keychain::init() {
        tracing::warn!(%error, "secure storage unavailable; tokens will not persist");
    }

    let mut builder = tauri::Builder::default();

    // Single-instance MUST be registered first (Tauri's own guidance: "must be
    // the first one to be registered to work well. This assures that it runs
    // before other plugins can interfere."). It is also what re-emits a
    // deep-link event on the surviving instance, via its `deep-link` feature.
    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            if let Some(window) = app.get_webview_window("main") {
                let _ = window.set_focus();
            }
        }));
    }

    let mut builder = builder
        .plugin(tauri_plugin_deep_link::init())
        .plugin(tauri_plugin_os::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_notification::init())
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_fs::init())
        .invoke_handler(tauri::generate_handler![
            commands::health_ping,
            commands::deep_link_current,
            commands::secret_set,
            commands::secret_get,
            commands::secret_delete,
            sync::commands::sync_configure,
            sync::commands::sync_status,
            sync::commands::sync_now,
            sync::commands::sync_read,
            sync::commands::sync_write,
            sync::commands::sync_tombstone,
            sync::commands::sync_reset,
            sync::commands::local_state_get,
            sync::commands::local_state_set,
        ]);

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());
        // The updater is registered in Phase 15 alongside the signing keypair and
        // the release endpoint; it cannot function before those exist, and
        // registering it without config panics at startup.
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        builder = builder.plugin(tauri_plugin_haptics::init());
    }

    builder
        .setup(|app| {
            #[cfg(desktop)]
            {
                use tauri_plugin_deep_link::DeepLinkExt;
                // Registers the `stvdy://` scheme at runtime for dev builds; in a
                // bundled app the scheme comes from the bundle metadata.
                let _ = app.deep_link().register_all();
            }

            {
                use tauri::Emitter;
                use tauri_plugin_deep_link::DeepLinkExt;
                // The plugin emits `deep-link://new-url` with raw URLs whenever a
                // URL arrives while the app is running. Re-emit the *classified*
                // intent instead, so no module outside `platform/` ever parses a
                // URL and the SPA never sees a raw string.
                let handle = app.handle().clone();
                app.deep_link().on_open_url(move |event| {
                    let intents: Vec<deeplink::DeepLinkIntent> = event
                        .urls()
                        .iter()
                        .map(|url| deeplink::classify(url.as_str()))
                        .collect();
                    for intent in &intents {
                        log_deep_link(intent, "event");
                    }
                    let _ = handle.emit(DEEP_LINK_EVENT, intents);
                });
            }

            {
                use tauri_plugin_deep_link::DeepLinkExt;
                // A cold start hands the launch URL to the plugin before this hook
                // runs (macOS/iOS via `RunEvent::Opened`, Windows/Linux via the CLI
                // arguments the plugin reads at init), so the listener above never
                // sees it. `get_current` is the documented cold-start path and the
                // SPA reads it through `deep_link_current` on boot; logging it here
                // makes that leg observable.
                if let Ok(Some(urls)) = app.deep_link().get_current() {
                    for url in &urls {
                        log_deep_link(&deeplink::classify(url.as_str()), "launch");
                    }
                }
            }

            // The local cache, and the sync shell that keeps it in step. A failure here is
            // not fatal: the app still runs online through the REST routes, so it is logged
            // and the sync commands report that they have no cache rather than the app
            // refusing to start. The cache is opened on every launch, which is why
            // `cache::open` is idempotent.
            {
                let notifier: sync::Notifier = {
                    let handle = app.handle().clone();
                    std::sync::Arc::new(move |notice| sync::commands::emit(&handle, notice))
                };
                let shell = match app.path().app_data_dir() {
                    Ok(directory) => match cache::open(&directory.join(CACHE_FILE)) {
                        Ok(connection) => match cache::SqliteStore::new(connection) {
                            Ok(store) => Some(std::sync::Arc::new(sync::SyncShell::new(
                                store, notifier,
                            ))),
                            Err(error) => {
                                tracing::warn!(%error, "the local cache is unusable; sync will not run");
                                None
                            }
                        },
                        Err(error) => {
                            tracing::warn!(%error, "the local cache could not be opened; sync will not run");
                            None
                        }
                    },
                    Err(error) => {
                        tracing::warn!(%error, "there is no app data directory; sync will not run");
                        None
                    }
                };
                app.manage(sync::commands::ShellState { shell });
            }

            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Stvdy shell");
}

/// Logs only the action label: a URL query string can carry an OAuth `code`,
/// which is a credential, so the URL itself is never logged.
fn log_deep_link(intent: &deeplink::DeepLinkIntent, stage: &str) {
    tracing::info!(action = intent.action(), stage, "deep link received");
}
