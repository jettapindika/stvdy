pub mod cache;
pub mod commands;
pub mod deeplink;
pub mod keychain;
pub mod push;
pub mod share;
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
        // Notifications, on every platform, from one plugin (A310 as revised): the
        // community `tauri-plugin-notifications` is a superset of the official
        // `tauri-plugin-notification`, and the official one cannot receive remote push —
        // its iOS delegate skips `UNPushNotificationTrigger` and its Android manifest
        // registers no FCM service. The two cannot coexist at all: both declare the
        // Android namespace `app.tauri.notification` and AGP 9 refuses to merge a
        // manifest two libraries claim, so linking both means **no APK can be
        // assembled**. Registered unconditionally now, because the local-notification
        // surface it also serves exists on the desktop too.
        .plugin(tauri_plugin_notifications::init())
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
            push::push_token,
            share::share_pending,
            share::share_clear,
        ]);

    #[cfg(desktop)]
    {
        builder = builder.plugin(tauri_plugin_global_shortcut::Builder::new().build());
        // The updater, registered here rather than in the common block because the
        // plugin's own platform matrix is `ios: none, android: none` (A335): mobile
        // updates ship through TestFlight and Play, and a mobile build that linked
        // it would fail to compile rather than silently expose an API that does
        // nothing. The permission is desktop-only for the same reason
        // (`capabilities/desktop.json`), and a plugin registered without it is a
        // runtime failure that `cargo test` cannot see.
        //
        // This plugin panics at startup with an absent `plugins.updater` config —
        // `Config.pubkey` has no serde default — so the config, the dependency and
        // this line are one change, which is why Phase 0 left all three out (A35).
        builder = builder.plugin(tauri_plugin_updater::Builder::new().build());
    }

    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        builder = builder.plugin(tauri_plugin_haptics::init());
    }

    // The shell's pass, handed to the mobile plugin below (A311/A315).
    //
    // The plugin cannot name the shell crate — the dependency runs the other way (the
    // shell registers the plugin), so a path dependency back would be a cycle cargo
    // refuses with `error: cyclic package dependency: package `stvdy-shell` depends on
    // itself`. This cell is the handover instead: the closure is built here, the shell
    // fills it in `setup` once the cache is open, and the pass reads it on every call
    // rather than capturing a value — a background launch may run before `setup`, and
    // the session changes under it whenever the SPA reconfigures.
    //
    // `OnceLock` rather than `LazyLock`: the value comes from runtime input (the cache
    // open in `setup`), which is exactly the case where the initializer cannot live with
    // the declaration. Declared outside the `cfg` below so the `setup` closure can see
    // it on every platform; only the mobile block ever fills or reads it.
    #[cfg(any(target_os = "ios", target_os = "android"))]
    let background_pass: std::sync::Arc<
        std::sync::OnceLock<Option<std::sync::Arc<sync::SyncShell>>>,
    > = std::sync::Arc::new(std::sync::OnceLock::new());

    #[cfg(any(target_os = "ios", target_os = "android"))]
    {
        // The custom mobile plugin (A311): the `BGTaskScheduler` / `WorkManager`
        // background seam and the Android share intake. Registered here for the same
        // reason it is a plugin at all — its `setup()` is the only hook that runs
        // before the launch sequence ends, which is when a `BGTaskScheduler` handler
        // must be registered (a launch the scheduler woke for an unregistered
        // identifier kills the app).
        let pass_cell = background_pass.clone();
        builder = builder.plugin(tauri_plugin_stvdy_mobile::init(std::sync::Arc::new(
            move || {
                let cell = pass_cell.clone();
                Box::pin(async move {
                    let Some(shell) = cell.get().and_then(Option::as_ref) else {
                        // The cache could not be opened, or this is a background launch
                        // that beat `setup`. Either way the honest answer is the same:
                        // nothing was reached, and the next foreground sync is the
                        // remedy (A316).
                        return Err("the local cache is not available".to_string());
                    };
                    let report = shell.sync_once().await.map_err(|error| error.to_string())?;
                    Ok(tauri_plugin_stvdy_mobile::PassOutcome {
                        state: report.state.to_string(),
                        outbox: report.outbox,
                    })
                })
            },
        )));
    }

    let result = builder
        .setup(move |app| {
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

            // Hands the background pass the shell it needs, once. On a phone this is
            // what makes `BGAppRefreshTask`/`WorkManager` reach the engine; on a
            // desktop the closure is never called, because the plugin that holds it is
            // mobile-only. The cell is empty when the cache could not be opened, and
            // the pass then reports that rather than pretending to have synced.
            #[cfg(any(target_os = "ios", target_os = "android"))]
            {
                let shell = app.state::<sync::commands::ShellState>().shell.clone();
                if background_pass.set(shell).is_err() {
                    tracing::warn!("the background pass already has a shell; keeping the first");
                }
            }

            // The share inbox (A312). On iOS the Share Extension has already copied the
            // file into the App Group container by the time the app is launched or
            // resumed — the extension cannot open the app, so this is the only handover
            // there is. Reading it here covers a cold start; `RunEvent::Resumed` below
            // covers the warm one, where a share arrives while the app is backgrounded.
            share::announce(app.handle());

            Ok(())
        })
        .build(tauri::generate_context!());

    // The event loop is the whole app: if it cannot start there is nothing to continue
    // into, so this is the one place a hard exit is right. Logged and exited non-zero
    // rather than `expect`-ed, because a supervisor should see a failure rather than a
    // process that quietly vanished -- and because a panic message on stderr is not a
    // record anyone can search.
    let app = match result {
        Ok(app) => app,
        Err(error) => {
            tracing::error!(%error, "the Stvdy shell could not start");
            std::process::exit(1);
        }
    };

    // The callback form rather than `Builder::run`, because the share import has to
    // happen on resume as well as on setup: on a phone the app is usually *already
    // running* when a file is shared to it, so `setup` never runs again and the file
    // would sit in the inbox until the next cold launch. `Resumed` is the OS's own
    // "this app is in front now", and the import is a directory read — cheap enough to
    // run on every one of them.
    app.run(|handle, event| {
        if let tauri::RunEvent::Resumed = event {
            share::announce(handle);
        }
    });
}

/// Logs only the action label: a URL query string can carry an OAuth `code`,
/// which is a credential, so the URL itself is never logged.
fn log_deep_link(intent: &deeplink::DeepLinkIntent, stage: &str) {
    tracing::info!(action = intent.action(), stage, "deep link received");
}
