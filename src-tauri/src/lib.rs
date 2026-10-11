pub mod commands;
pub mod keychain;

use tauri::Manager;

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
            commands::secret_set,
            commands::secret_get,
            commands::secret_delete,
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
            Ok(())
        })
        .run(tauri::generate_context!())
        .expect("error while running the Stvdy shell");
}
