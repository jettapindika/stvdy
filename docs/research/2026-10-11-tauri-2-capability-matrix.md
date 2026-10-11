# Tauri 2 capability matrix — macOS + Windows + iOS + Android from one codebase

**Verified 2026-10-11.** Every version and platform claim below was fetched live today from crates.io, the npm registry, raw GitHub sources (`plugins-workspace@v2`, `tauri@dev`, `tauri-docs@v2`) and Apple / Android / Microsoft developer documentation. Items that could not be confirmed from a primary source are marked **UNVERIFIED**.

Headline answers:

- **Mobile support is GA, not beta.** iOS and Android shipped as stable, first-class targets in **Tauri 2.0 stable on 2024-10-02**. There is no beta flag to opt into.
- **The plugin layer covers most of what a desktop+mobile SPA needs** (deep links, local + scheduled notifications, dialogs, files, SQLite, HTTP, logging).
- **Three things genuinely need custom Swift/Kotlin:** the iOS share-sheet *destination*, OS background schedulers for the sync engine, and (optionally) APNs/FCM plumbing.
- **One thing is impossible and must be designed around:** running the SPA's JavaScript while the app is backgrounded or terminated.

---

## 1. Package / crate matrix

Platform abbreviations: **mac** = macOS, **win** = Windows, **lin** = Linux, **and** = Android, **ios** = iOS. "partial" and "none" reproduce Tauri's own `[package.metadata.platforms.support]` levels.

### 1.1 Core

| Package / crate | Latest stable version | Platforms | Notes |
|---|---|---|---|
| `tauri` | **2.12.3** (2026-10-10) | mac, win, lin, ios, and | Stable GA. Mobile GA since 2.0 (2024-10-02). MSRV **1.90**, edition 2024. `3.0.0-alpha.4` exists (2026-10-01); **no 3.0 stable**. |
| `tauri-build` | **2.7.2** (2026-10-10) | all | Build-time codegen: injects Android file-association intent filters into `AndroidManifest.xml`, writes iOS entitlements / `Info.plist`. |
| `tauri-cli` / `@tauri-apps/cli` | **2.12.2** (2026-10-10) | all | `next` dist-tag = `3.0.0-alpha.4`. |
| `@tauri-apps/api` | **2.12.3** (2026-10-10) | all | `next` dist-tag = `3.0.0-alpha.2`. |

### 1.2 Official plugins (the ones in scope)

| Package / crate | Latest stable version | Platforms | Notes |
|---|---|---|---|
| `tauri-plugin-deep-link` / `@tauri-apps/plugin-deep-link` | **2.6.1** (2026-10-01) | win, lin **full**; mac, and, ios **partial** | Mobile is **config-only registration**: "Deep links must be registered in config. Dynamic registration at runtime is not supported." `register()` / `unregister()` / `isRegistered()` return `UnsupportedPlatform` on mac/and/ios. `build.rs` generates the Android `<intent-filter>` (with `android:autoVerify="true"` for App Links) and, on Apple, the `com.apple.developer.associated-domains` entitlement plus `CFBundleURLTypes` for custom schemes. Universal Links need `https://<host>/.well-known/apple-app-site-association`; Android App Links need `https://<host>/.well-known/assetlinks.json`. |
| `tauri-plugin-notification` / `@tauri-apps/plugin-notification` | **2.6.0** (2026-10-10) | mac, win, lin, and, ios — **all full** | Supports **scheduled local notifications that fire while the app is closed and offline**. iOS → `UNUserNotificationCenter` (`UNTimeIntervalNotificationTrigger` / `UNCalendarNotificationTrigger`); Android → `AlarmManager` with `BOOT_COMPLETED` + `LOCKED_BOOT_COMPLETED` restore receivers. Desktop (notify-rust) **ignores** scheduling. **Local only — cannot receive APNs/FCM.** |
| `tauri-plugin-updater` / `@tauri-apps/plugin-updater` | **2.13.3** (2026-10-10) | mac, win, lin **full**; and, ios **none** | **Desktop-only.** Produces macOS `.app.tar.gz`, Windows `msi`/`nsis`, Linux AppImage updater bundles. Mobile must use App Store / Play Store. |
| `tauri-plugin-dialog` / `@tauri-apps/plugin-dialog` | **2.8.2** (2026-10-10) | mac, win, lin **full**; and, ios **partial** | Android/iOS note: "Does not support folder picker" (`open({directory:true})` → `Error::FolderPickerNotImplemented`). Android uses `ACTION_GET_CONTENT` / `ACTION_CREATE_DOCUMENT` and returns `content://` URIs; iOS uses `UIDocumentPickerViewController`, returning `file://` URLs (copy into sandbox by default, `fileAccessMode:'scoped'` for open-in-place). |
| `tauri-plugin-fs` / `@tauri-apps/plugin-fs` | **2.7.0** (2026-10-10) | mac, win, lin **full**; and, ios **partial** | Mobile: "Access is restricted to Application folder by default". Handles Android `content://` for content commands and auto-manages iOS security-scoped resources. |
| `tauri-plugin-opener` / `@tauri-apps/plugin-opener` | **2.7.1** (2026-10-10) | mac, win, lin **full**; and, ios **partial** | Mobile: "Only allows to open URLs via `open`". |
| `tauri-plugin-os` / `@tauri-apps/plugin-os` | **2.4.0** (2026-09-30) | all five **full** | `next` = `3.0.0-alpha.2`. |
| `tauri-plugin-global-shortcut` / `@tauri-apps/plugin-global-shortcut` | **2.4.1** (2026-10-10) | mac, win, lin **full**; and, ios **none** | **Desktop-only.** |
| `tauri-plugin-single-instance` | **2.5.3** (2026-10-10) | mac, win, lin **full**; and, ios **none** | **Desktop-only.** Rust-only, **no npm package**. `deep-link` feature wires desktop deep links into a single instance. |
| `tauri-plugin-sql` / `@tauri-apps/plugin-sql` | **2.5.0** (2026-09-30) | all five **full** | Wraps **sqlx 0.8** (pinned). SQLite path is relative to the **app config dir**; supports Rust-side `Migration` / `MigrationKind` + `preload`. All migrations run inside one transaction. |
| `tauri-plugin-store` / `@tauri-apps/plugin-store` | **2.5.0** (2026-09-30) | all five **full** | **Plaintext pretty-printed JSON** written with `fs::write`. No crypto dependency, no OS-encryption option. |
| `tauri-plugin-stronghold` / `@tauri-apps/plugin-stronghold` | **2.4.0** (2026-09-30) | metadata claims all five | **Officially deprecated** (see §3/§4). |
| `tauri-plugin-biometric` / `@tauri-apps/plugin-biometric` | **2.4.2** (2026-10-10) | ios, and only | Prompt/gate only — **stores nothing**. |

### 1.3 Secure storage candidates

| Package / crate | Latest stable version | Platforms | Notes |
|---|---|---|---|
| `keyring` | **4.2.0** (2026-08-29) | mac, win, lin with default features | 4.x is a **meta-crate** over `keyring-core` + per-platform store crates. Default feature `v1` = apple keychain + windows-native + zbus secret-service **only**. |
| `keyring-core` | **1.0.0** (2026-04-21) | — (facade) | ~3.6M recent downloads. |
| `apple-native-keyring-store` | **1.0.2** (2026-08-06) | mac, ios | iOS = `protected` (data-protection) store; macOS = `keychain` + `protected`. |
| `windows-native-keyring-store` | **1.1.0** (2026-05-24) | win | Each entry maps to a `CRED_TYPE_GENERIC` credential; default persistence `Enterprise`. |
| `android-native-keyring-store` | **1.0.0** (2026-04-21) | and | SharedPreferences + Android Keystore via JNI. |
| `tauri-plugin-keyring-store` / `tauri-plugin-keyring-store-api` | **0.2.0** (2026-05-22) | mac, win, lin, ios, and | Community (s00d) wrapper over the four crates above. Works on all four platforms today. Single maintainer. |
| `tauri-plugin-keyring` / `tauri-plugin-keyring-api` | **0.1.0 / 0.1.1** (both **2024-12-23**) | mac, win, ios, lin — **no Android** | Wraps `keyring` v3; Android falls through to a mock store. ~22 months stale. |
| `tauri-plugin-stronghold` | **2.4.0** (2026-09-30) | metadata claims all five | **DEPRECATED** — plugins-workspace #3494 (2026-07-16): "will not be developed further… will not be available in Tauri v3… upstream `stronghold` rust crate… has been unmaintained for a long time." |
| `iota_stronghold` (upstream of the above) | **2.1.0** (**2024-05-13**) | — | No release in ~2.4 years. |
| `tauri-plugin-store` | **2.5.0** | all five | Plaintext — **never for tokens**. |

### 1.4 Local SQLite candidates

| Package / crate | Latest stable version | Platforms | Notes |
|---|---|---|---|
| `rusqlite` | **0.40.2** (2026-08-08) | mac, win, lin, ios, and | Sync API. `bundled` feature compiles **SQLite 3.53.2** from source — no system dependency, cross-compiles to iOS/Android. |
| `sqlx` | **0.9.0** (2026-05-21) | mac, win, lin, ios, and | Async. 0.9 renamed SQLite features to `sqlite-bundled` / `sqlite-unbundled`; MSRV **1.94**. `tauri-plugin-sql` still pins **0.8**. |
| `libsql` | **0.9.30** (2026-06-02) | mac, win, lin, ios, and | Fork of rusqlite with optional embedded replica + encryption at rest. `0.10.0-pre.4` on the newest tag. |

### 1.5 Push notification candidates (no official plugin exists)

| Package / crate | Latest stable version | Platforms | Notes |
|---|---|---|---|
| *(official)* | — | — | **No official APNs/FCM plugin.** Not in `plugins-workspace/v2/plugins`, not on the official plugin list, not in awesome-tauri. |
| `tauri-plugin-notifications` / `@choochmeque/tauri-plugin-notifications-api` | crate **0.5.0-rc.14** (2026-09-16), latest non-prerelease **0.4.6**; npm **0.5.0-rc.14** | ios, and, mac, lin (win "not yet available") | **Most active community option.** APNs (iOS/macOS) + FCM (Android) + UnifiedPush (Linux). Swizzles the iOS AppDelegate; ships its own Android `FirebaseMessagingService`. **No custom Swift/Kotlin needed.** Still pre-1.0. |
| `tauri-plugin-mobile-push` / `tauri-plugin-mobile-push-api` | **0.1.4** (2026-04-18) | ios 13+, and 7+ (API 24) | README states **iOS event listeners are not functional**; Android needs a manual manifest `<service>`. Low maintenance (~1.4k recent downloads). |
| `tauri-plugin-fcm` / `tauri-plugin-fcm` | **0.2.0** (2026-05-06) | ios, and | Returns an **FCM** token on both platforms (iOS does the APNs→FCM exchange). Setup is plist/json/Gradle only. No documented background JS event. |
| `tauri-plugin-remote-push` / `tauri-plugin-remote-push-api` | **1.0.10** (2025-06-23) | ios, and | **Stale (~16 months).** **Requires custom Swift** (edit `AppDelegate.swift`) + manual Android FCM service. Repository URL is a placeholder — no discoverable repo. |
| `tauri-plugin-push-notifications` | **0.1.0** (2026-07-14) | ios, and | Very new, ~199 recent downloads. |
| `tauri-plugin-background-service` | **1.0.1** (2026-07-30) | ios, and (+ desktop tokio) | Community, 6 stars. Android foreground service + iOS `BGTaskScheduler` lifecycle around a Rust trait. |

### 1.6 Mobile toolchain

| Requirement | Value | Notes |
|---|---|---|
| iOS deployment target | **15.0** (default) | `bundle.iOS.minimumSystemVersion`; override in `tauri.conf.json`. |
| iOS build tooling | Xcode + **xcodegen** + **CocoaPods** (`pod install`) | CLI installs the iOS rust targets itself. macOS-only. |
| iOS rust targets | `aarch64-apple-ios`, `x86_64-apple-ios`, `aarch64-apple-ios-sim` | `rustup target add …`. |
| Android `minSdk` | **24** (Android 7.0 Nougat) | `bundle.android.minSdkVersion`. |
| Android `compileSdk` / `targetSdk` | **37** | Raised from 36 to 37 (Android 17) in the 2.12 template. |
| Android NDK | **29.0.13846066** | Auto-installed via `sdkmanager` if missing. |
| Gradle / AGP / Kotlin | **9.6.1** / **9.3.1** / **2.2.10** | Minimum supported Gradle is **8.13**. Java up to **25**. |
| Android rust targets | `aarch64-linux-android`, `armv7-linux-androideabi`, `i686-linux-android`, `x86_64-linux-android` | |
| 16 KB memory pages | NDK ≥ 28 handles it; otherwise add `-C link-arg=-Wl,-z,max-page-size=16384` to `.cargo/config.toml` | Google requirement for new Play submissions. |
| Rust MSRV | **1.90** | Raised in Tauri 2.12; plugins-workspace uses the same. |
| Tauri updater on mobile | **Not supported** | `android: none`, `ios: none`. |

---

## 2. Needs custom Swift/Kotlin

Each item: what the official layer lacks, and what the native shim must do.

1. **iOS Share Sheet destination** (your app appearing in the share sheet for a PDF from Files / Google Drive).
   - *Lacks:* Tauri can register your app as a *file handler* (`CFBundleDocumentTypes` / `LSItemContentTypes` via `bundle.fileAssociations[]`) — that is the "Open in…" document menu. It has no support for a **Share Extension**, which is the mechanism that puts an app row in the activity/share sheet. Tauri also has no `bundle` key for app-extension targets.
   - *Shim:* a separate **Share Extension target** with `NSExtension { NSExtensionPointIdentifier = com.apple.share-services, NSExtensionPrincipalClass, NSExtensionAttributes.NSExtensionActivationRule }`. A Share Extension **cannot launch its host app** (`NSExtensionContext.open(_:completionHandler:)` is documented as supported only by the Today and iMessage extension points), so you need an **App Group shared container**: the extension writes the file/URL into the group container, the main app imports it on next foreground. Requires custom Swift on both sides plus the App Group entitlement. The escape hatch for target wiring is `bundle.iOS.template` (a custom XcodeGen `project.yml` rendered over the generated one); where cargo-mobile2's `additional-targets` is declared is **UNVERIFIED**.

2. **iOS background execution for the sync engine.**
   - *Lacks:* `BGTaskScheduler.shared.register(...)` must happen before the end of the app launch sequence, and the silent-push / background-URLSession callbacks live in the AppDelegate. Tauri's documented mobile plugin lifecycle hooks are only **`load`** (both platforms) and **`onNewIntent`** (**Android only**). There is **no hook for app-delegate callbacks or `BGTaskScheduler` registration**.
   - *Shim:* hand-wire the generated Xcode project (or a plugin that installs its own delegate / notification-center delegate). Implement `application(_:didReceiveRemoteNotification:fetchCompletionHandler:)` (≈30 s budget) and `application(_:handleEventsForBackgroundURLSession:completionHandler:)`, plus `BGAppRefreshTaskRequest` / `BGProcessingTaskRequest` submission and resubmission from inside each launch handler. Add `BGTaskSchedulerPermittedIdentifiers` and `UIBackgroundModes` to `Info.plist` and enable the Background Modes capability.

3. **Android background execution for the sync engine.**
   - *Lacks:* no official plugin provides a `WorkManager` worker, `JobService`, or FCM service hook. The feature request `[Mobile] Add native task scheduler support` was closed and converted to a discussion; the maintainer states they cannot take on more maintenance burden.
   - *Shim:* a `CoroutineWorker` / `JobService` + `WorkManager` enqueue, an FCM `FirebaseMessagingService`, and — if you need prompt work — a foreground service with `android:foregroundServiceType="dataSync"` + `FOREGROUND_SERVICE_DATA_SYNC` + `POST_NOTIFICATIONS`. This is a **plugin**, not a fork: a Tauri plugin can ship its own `android/src/main/AndroidManifest.xml` (the official notification plugin declares receivers and permissions exactly that way). The plugin's `.setup()` hook is the seam for running Rust work while the process lives.

4. **Android share-OUT** (your app → other apps).
   - *Lacks:* no plugin exposes `ACTION_SEND`. The generated manifest already declares `androidx.core.content.FileProvider` (`${applicationId}.fileprovider`, `@xml/file_paths`), but there is no JS share API.
   - *Shim:* Kotlin `Intent(ACTION_SEND)` + the existing FileProvider.

5. **Android folder picking and durable picked-file access.**
   - *Lacks:* `open({directory:true})` throws `Error::FolderPickerNotImplemented` on mobile. The dialog plugin deliberately uses `ACTION_GET_CONTENT` and never calls `takePersistableUriPermission`, so the grant is transient (it lasts until the receiving activity finishes).
   - *Shim:* `ACTION_OPEN_DOCUMENT_TREE` for folder picking and `ACTION_OPEN_DOCUMENT` + `takePersistableUriPermission` if you need durable access — or simply **copy the file into your own storage immediately**.

6. **Biometric-bound secrets.**
   - *Lacks:* `tauri-plugin-biometric` only prompts; no maintained plugin stores a secret *behind* biometrics.
   - *Shim:* Kotlin `setUserAuthenticationRequired(true)` / Swift `SecAccessControl`, plus a re-auth path for Android's biometric-enrollment key invalidation (see §3).

7. **APNs/FCM native configuration** (config, not code — but hand-done).
   - iOS: `aps-environment` entitlement + Push Notifications + Background Modes → Remote notifications capabilities. The Tauri-generated Xcode project does not add these.
   - Android: `google-services.json`, the Google Services Gradle plugin, Firebase BOM + `firebase-messaging`, an FCM `<service>` with `com.google.firebase.MESSAGING_EVENT`, and `POST_NOTIFICATIONS`.
   - *Which plugins avoid code:* `tauri-plugin-notifications` (Choochmeque), `tauri-plugin-mobile-push` and `tauri-plugin-fcm` need **no custom Swift/Kotlin** (config only). `tauri-plugin-remote-push` **does** need custom Swift. The SableClient fork needs a Notification Service Extension for background decryption.

8. **Rolling your own Android crypto / backup exclusion.**
   - `androidx.security-crypto`'s `EncryptedSharedPreferences` is **deprecated** in 1.1.0, so the 2026 path is Keystore + AES-GCM by hand. Tauri's Android template sets no `android:allowBackup` and no `dataExtractionRules`, so Auto Backup includes your app dir by default and a restored install loses Keystore keys.

---

## 3. Not possible / design around

1. **Running the SPA's JavaScript while backgrounded or terminated.**
   - *Constraint:* On iOS a suspended app runs no code, and the `WKWebView` renders in a **separate web-content process** that can be terminated at any time; there is no `UIBackgroundMode` for web content. On Android the process is a *cached* process that can be killed, and "an app process may receive limited or no execution time" until it re-enters an active lifecycle state.
   - *Workaround:* put the sync engine in **Rust/native** behind a native scheduler seam. JS only observes and replays state when the app is foregrounded. No plugin, official or community, provides headless/background JS.

2. **Guaranteed periodic background sync.**
   - *Constraint:* iOS `BGTaskScheduler` scheduling is explicitly **discretionary** ("The system decides the best time to launch your background task"). Android `WorkManager` periodic work has a **15-minute minimum**, is inexact, is deferred in Doze, and is capped by standby-bucket quotas — as little as **10 minutes per rolling 24 h** in the rare bucket and once/day in restricted.
   - *Workaround:* foreground sync on open/resume is the only fully reliable window; add one `BGAppRefreshTask` (≤30 s) + one `BGProcessingTask` on iOS, one unique periodic `WorkManager` job (≥15 min) on Android, and treat push as an opportunistic wake-up, never a schedule.

3. **Durable access to a file the user picked with `dialog.open()` on Android.**
   - *Constraint:* `ACTION_GET_CONTENT` grants are transient and the plugin never calls `takePersistableUriPermission`.
   - *Workaround:* **copy the file into your own storage immediately** on pick.

4. **Android path commands on a `content://` URI.**
   - *Constraint:* `fs.stat` / `exists` / `size` / `readDir` / `rename` / `remove` / `create` all go through `resolve_path`, which fails via `Url::to_file_path()`. Only URL-based content commands (`readFile`, `readTextFile`, `open`, `writeFile`, `writeTextFile`) work, because they bypass the path scope.
   - *Workaround:* use URL-based APIs for picked/shared files; never `stat` them.

5. **iOS "Share → my app" implemented purely with a deep link.**
   - *Constraint:* a file share cannot be expressed as a URL of your scheme; the OS routes files through document types (iOS) / intents (Android).
   - *Workaround:* a Share Extension (§2.1). A deep-link-only flow would require the user to copy a shareable link, and only works for cloud content with a URL.

6. **Reliable silent/background push delivery.**
   - *Constraint:* iOS does not guarantee delivery, throttles to roughly **2–3 per hour**, keeps only the newest held notification, and discards it if the app is force-quit; the app gets ≈30 s to run.
   - *Workaround:* treat silent push as a hint, not a guarantee; always re-sync on foreground.

7. **A Tauri-provided mobile update path.**
   - *Constraint:* `tauri-plugin-updater` marks `android: none`, `ios: none` and only produces macOS `.app.tar.gz`, Windows `msi`/`nsis` and Linux AppImage bundles.
   - *Workaround:* ship mobile updates through the App Store / Play Store.

8. **Any background execution after the user force-quits.**
   - *Constraint:* iOS discards the held notification; Android's Task Manager "Stop" kills the whole app (jobs and alarms still fire at their scheduled times).
   - *Workaround:* accept it; do not rely on force-quit-time work.

9. **Minute-accurate local notifications on Android out of the box.**
   - *Constraint:* the notification plugin checks `canScheduleExactAlarms()` and silently falls back to inexact `setAndAllowWhileIdle` on API 31+, and its manifest declares only `RECEIVE_BOOT_COMPLETED`, `WAKE_LOCK`, `POST_NOTIFICATIONS` — no `SCHEDULE_EXACT_ALARM`.
   - *Workaround:* add `SCHEDULE_EXACT_ALARM` (user-grantable, revocable) yourself if you need minute accuracy, and design reminders so inexact delivery is acceptable.

10. **Storing a large secret in Windows Credential Manager.**
    - *Constraint:* `CredentialBlobSize` cannot exceed `CRED_MAX_CREDENTIAL_BLOB_SIZE` = **2560 bytes**.
    - *Workaround:* store a refresh token (small), not a serialized session blob; keep bulk state in SQLite.

---

## 4. Recommendations

### (a) Secure token storage

**The only approach that works on all four platforms today** is the OS credential store via the keyring ecosystem:

- **`keyring-core` 1.0.0** + **`apple-native-keyring-store` 1.0.2** (`protected` on iOS, `keychain` on macOS) + **`windows-native-keyring-store` 1.1.0** + **`android-native-keyring-store` 1.0.0** — wired yourself with per-`cfg` store construction (~30 lines), or via the community **`tauri-plugin-keyring-store` 0.2.0**, which does exactly this.
- **`keyring` 4.2.0 with default features is DESKTOP-ONLY.** Its default feature `v1` builds only macOS / Windows / non-Android-unix stores; on iOS and Android `Entry::new` returns `Error::NoDefaultStore`. The only bundled feature set that includes Android is `cli`, which the crate's own docs tell applications not to use. So: **iOS/Android do not work with the default `keyring` configuration** — you must depend on the per-platform store crates directly (or use the plugin).
- Avoid: **`tauri-plugin-stronghold`** (officially deprecated, will not exist in v3; it is an encrypted snapshot *file* + password, not an OS keychain), **`tauri-plugin-keyring` 0.1.0** (no Android, stale since 2024-12), **`tauri-plugin-store`** (plaintext).
- Caveats: Windows 2560-byte blob limit; iOS keychain items survive uninstall (delete on first launch if "reinstall = fresh login"); exclude secret files from Android Auto Backup or accept re-auth after restore; Android Keystore keys bound to biometrics are irreversibly invalidated when a new biometric is enrolled; macOS prompts only when the caller is not already trusted (your own signed app reads silently).

### (b) Push notifications (APNs / FCM)

- **There is no official Tauri push plugin**, and the official `tauri-plugin-notification` **provably cannot receive remote push** — its iOS delegate explicitly skips `UNPushNotificationTrigger`, and its Android manifest registers no `com.google.firebase.MESSAGING_EVENT` service.
- **Pick:** Choochmeque's **`tauri-plugin-notifications` 0.5.0-rc.14** (APNs + FCM + UnifiedPush, no custom Swift/Kotlin, active development — last commit 2026-10-04). It is pre-1.0 and single-maintainer, so pin it and be ready to vendor.
- **Alternatives:** `tauri-plugin-fcm` 0.2.0 (FCM on both platforms via APNs→FCM exchange, config-only); `tauri-plugin-mobile-push` 0.1.4 (low maintenance, iOS event listeners broken).
- **Custom native fallback:** budget for it regardless — iOS needs `aps-environment` + Background Modes, Android needs `google-services.json` + the Google Services Gradle plugin + an FCM service. A custom plugin is the supported place for both.
- **Background push:** plugins can *receive* a silent payload in native code, but **none can run the SPA's JS** while backgrounded or terminated. Tap-to-open (including cold-start buffering) is the practical JS surface.

### (c) Local SQLite in the Rust shell

- **Pick `rusqlite` 0.40.2 with the `bundled` feature** for an app-owned cache + outbox. It is synchronous (fine — the outbox writes are short), carries its own static SQLite 3.53.2, and cross-compiles cleanly to `aarch64-apple-ios` / `aarch64-linux-android` with no system dependency. This is the right call if the cache is owned by the Rust shell and you want migrations under your own control.
- **Choose `sqlx` 0.9.0** instead if you want to share async code and row types with the Axum server; use `sqlite-bundled`.
- **Use `tauri-plugin-sql` 2.5.0 only if you need to query from JS.** It pins **sqlx 0.8**, so mixing it with sqlx 0.9 in the same binary duplicates the driver stack. Its SQLite path is relative to the app **config** dir, and its Rust-side `Migration`/`MigrationKind` + `preload` is a reasonable migration story.
- Mobile pitfalls: resolve paths at runtime (iOS sandbox paths change between installs — never persist absolute paths); keep the DB in Application Support / app data, not Caches; apply `NSURLIsExcludedFromBackupKey` to the DB and its `-wal`/`-shm` siblings if the cache must not go to iCloud (no official plugin does this — small custom Swift/plugin task); keep the outbox transactional and resumable because both platforms can kill a background window mid-sync.

### (d) Scheduled local notifications for offline reminders

- **Use the official `tauri-plugin-notification` 2.6.0** — it genuinely fires while the app is closed and offline on both mobile platforms (Android `AlarmManager` + `BOOT_COMPLETED`/`LOCKED_BOOT_COMPLETED` restore receivers; iOS `UNUserNotificationCenter`).
- API: `sendNotification({ …, schedule: Schedule.at(date, repeating, allowWhileIdle) })`, `Schedule.interval({hour, minute})`, `Schedule.every(ScheduleEvery.Hour, n)`, plus `pending()`, `cancel(ids)`, `cancelAll()` (mobile-only).
- Gotchas: **desktop ignores scheduling entirely**; iOS repeating intervals must be at least **1 minute**; Android falls back to inexact alarms unless you add `SCHEDULE_EXACT_ALARM`; iOS has a cap on pending scheduled notifications (commonly cited as 64 — **UNVERIFIED**).

### (e) File picker + iOS/Android share-sheet import

- **File picker:** official `tauri-plugin-dialog` 2.8.2 + `tauri-plugin-fs` 2.7.0, no native code. Use `open({filters, pickerMode, multiple, fileAccessMode})`; read the result with the **URL-based** fs APIs. On Android, copy immediately for durable access. No folder picker on mobile.
- **Import from Files/Drive (Android):** **config-only.** Declare `bundle.fileAssociations[]` with a concrete `mimeType`; `tauri-build` injects `SEND` / `SEND_MULTIPLE` / `VIEW` intent filters, and `RunEvent::Opened { urls }` delivers the `content://` URI, which the fs plugin can read. Caveats: the generated filters match on **MIME type only** (the `pathPattern`s are inert without a scheme), a filter generated without a `mimeType` can never match a real intent, and `mimeType: "*/*"` does **not** match concrete types — always set a concrete MIME type.
- **Import from Files/Drive (iOS):** document associations (`bundle.fileAssociations[]` → `CFBundleDocumentTypes` / `LSItemContentTypes`) cover **"Open in…"**, i.e. opening a file with your app. A true **share-sheet row needs a Share Extension** (custom Swift + App Group), see §2.1. iOS cold-start delivery of `RunEvent::Opened` under the scene lifecycle is **UNVERIFIED** (warm start is implemented); test on device.
- **Deep link alternative:** only for links, not files — `plugins.deepLink.mobile[{scheme, appLink:false}]` + `onOpenUrl` / `getCurrent`. Not a substitute for file intake.

### (f) Sync engine shape (implied by §3)

Put the engine in Rust behind a native scheduler seam. Foreground sync on open/resume is the only fully reliable window; add one `BGAppRefreshTask` (≤30 s) + one `BGProcessingTask` on iOS, one unique periodic `WorkManager` job (≥15 min) on Android, and treat push as an opportunistic wake-up. No official plugin provides this hook, so a custom mobile plugin (or hand-edited Xcode/Gradle project) is required. Budget the iOS Share Extension, the push integration and the background scheduler as **one native-work package** — these will dominate your mobile effort; the plugin/SPA layer is the easy part.

---

## 5. Sources

All fetched 2026-10-11.

**Registries**

- https://crates.io/api/v1/crates/tauri
- https://crates.io/api/v1/crates/tauri-build
- https://crates.io/api/v1/crates/tauri-cli
- https://crates.io/api/v1/crates/tauri-plugin-deep-link
- https://crates.io/api/v1/crates/tauri-plugin-notification
- https://crates.io/api/v1/crates/tauri-plugin-updater
- https://crates.io/api/v1/crates/tauri-plugin-dialog
- https://crates.io/api/v1/crates/tauri-plugin-fs
- https://crates.io/api/v1/crates/tauri-plugin-opener
- https://crates.io/api/v1/crates/tauri-plugin-os
- https://crates.io/api/v1/crates/tauri-plugin-global-shortcut
- https://crates.io/api/v1/crates/tauri-plugin-single-instance
- https://crates.io/api/v1/crates/tauri-plugin-sql
- https://crates.io/api/v1/crates/tauri-plugin-store
- https://crates.io/api/v1/crates/tauri-plugin-stronghold
- https://crates.io/api/v1/crates/tauri-plugin-biometric
- https://crates.io/api/v1/crates/tauri-plugin-keyring
- https://crates.io/api/v1/crates/keyring
- https://crates.io/api/v1/crates/keyring-core
- https://crates.io/api/v1/crates/apple-native-keyring-store
- https://crates.io/api/v1/crates/android-native-keyring-store
- https://crates.io/api/v1/crates/windows-native-keyring-store
- https://crates.io/api/v1/crates/tauri-plugin-keyring-store
- https://crates.io/api/v1/crates/rusqlite
- https://crates.io/api/v1/crates/sqlx
- https://crates.io/api/v1/crates/libsql
- https://crates.io/api/v1/crates/tauri-plugin-notifications
- https://crates.io/api/v1/crates/tauri-plugin-mobile-push
- https://crates.io/api/v1/crates/tauri-plugin-fcm
- https://crates.io/api/v1/crates/tauri-plugin-remote-push
- https://crates.io/api/v1/crates/tauri-plugin-push-notifications
- https://crates.io/api/v1/crates/tauri-plugin-background-service
- https://crates.io/api/v1/crates/iota_stronghold
- https://registry.npmjs.org/@tauri-apps/api
- https://registry.npmjs.org/@tauri-apps/cli
- https://registry.npmjs.org/@tauri-apps/plugin-notification
- https://registry.npmjs.org/@tauri-apps/plugin-deep-link
- https://registry.npmjs.org/@tauri-apps/plugin-updater
- https://registry.npmjs.org/@choochmeque/tauri-plugin-notifications-api
- https://registry.npmjs.org/tauri-plugin-keyring-store-api
- https://registry.npmjs.org/tauri-plugin-keyring-api
- https://registry.npmjs.org/-/v1/search?text=tauri%20push%20notification

**Official Tauri docs**

- https://v2.tauri.app/plugin/
- https://v2.tauri.app/plugin/deep-linking/
- https://v2.tauri.app/plugin/notification/
- https://v2.tauri.app/plugin/updater/
- https://v2.tauri.app/plugin/sql/
- https://v2.tauri.app/plugin/file-system/
- https://v2.tauri.app/plugin/dialog/
- https://v2.tauri.app/plugin/stronghold/
- https://v2.tauri.app/plugin/global-shortcut/
- https://v2.tauri.app/plugin/single-instance/
- https://v2.tauri.app/learn/mobile-file-associations/
- https://v2.tauri.app/develop/plugins/
- https://v2.tauri.app/develop/plugins/develop-mobile/
- https://v2.tauri.app/start/prerequisites/
- https://v2.tauri.app/distribute/google-play/
- https://v2.tauri.app/distribute/app-store/
- https://v2.tauri.app/blog/tauri-2.0/
- https://v2.tauri.app/blog/tauri-2.12/

**Tauri / plugin source**

- https://github.com/tauri-apps/plugins-workspace/tree/v2/plugins
- https://github.com/tauri-apps/plugins-workspace/issues/3494 (Stronghold deprecation)
- https://github.com/tauri-apps/tauri/issues/14530 (native task scheduler request)
- https://github.com/orgs/tauri-apps/discussions/14531
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/Cargo.toml
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/Cargo.toml
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/CHANGELOG.md
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/android/src/main/AndroidManifest.xml
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/android/src/main/java/TauriNotificationManager.kt
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/android/src/main/java/NotificationSchedule.kt
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/ios/Sources/Notification.swift
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/ios/Sources/NotificationManager.swift
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/notification/guest-js/index.ts
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/deep-link/build.rs
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/deep-link/guest-js/index.ts
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/store/src/store.rs
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/dialog/guest-js/index.ts
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/fs/src/lib.rs
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/sql/Cargo.toml
- https://raw.githubusercontent.com/tauri-apps/plugins-workspace/v2/plugins/sql/src/wrapper.rs
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-build/src/mobile.rs
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-utils/src/config.rs
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri/src/path/mod.rs
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri/src/path/android.rs
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri/mobile/android/src/main/java/app/tauri/PathPlugin.kt
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/packages/api/src/path.ts
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-cli/src/mobile/android/mod.rs
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-cli/src/mobile/android/check_java_gradle_versions.rs
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-cli/templates/mobile/android/app/build.gradle.kts
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-cli/templates/mobile/android/build.gradle.kts
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-cli/templates/mobile/android/gradle/wrapper/gradle-wrapper.properties
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-cli/templates/mobile/android/app/src/main/AndroidManifest.xml
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-cli/templates/mobile/ios/project.yml
- https://raw.githubusercontent.com/tauri-apps/tauri/dev/crates/tauri-cli/templates/mobile/ios/Podfile
- https://raw.githubusercontent.com/tauri-apps/tauri-docs/v2/src/content/docs/learn/mobile-file-associations.mdx

**Upstream crates**

- https://raw.githubusercontent.com/open-source-cooperative/keyring-rs/main/Cargo.toml
- https://raw.githubusercontent.com/open-source-cooperative/keyring-rs/v4.2.0/src/v1.rs
- https://raw.githubusercontent.com/open-source-cooperative/android-native-keyring-store/main/README.md
- https://raw.githubusercontent.com/rusqlite/rusqlite/master/README.md
- https://raw.githubusercontent.com/launchbadge/sqlx/main/Cargo.toml
- https://raw.githubusercontent.com/dirs-dev/dirs-rs/main/src/mac.rs
- https://raw.githubusercontent.com/dirs-dev/dirs-sys-rs/main/src/lib.rs

**docs.rs**

- https://docs.rs/apple-native-keyring-store/latest/apple_native_keyring_store/
- https://docs.rs/android-native-keyring-store/latest/android_native_keyring_store/
- https://docs.rs/windows-native-keyring-store/latest/windows_native_keyring_store/

**Apple**

- https://developer.apple.com/documentation/backgroundtasks/choosing-background-strategies-for-your-app
- https://developer.apple.com/documentation/backgroundtasks/bgprocessingtaskrequest
- https://developer.apple.com/documentation/backgroundtasks/bgcontinuedprocessingtaskrequest
- https://developer.apple.com/documentation/backgroundtasks/performing-long-running-tasks-on-ios-and-ipados
- https://developer.apple.com/documentation/uikit/using-background-tasks-to-update-your-app
- https://developer.apple.com/documentation/usernotifications/pushing-background-updates-to-your-app
- https://developer.apple.com/documentation/foundation/downloading-files-in-the-background
- https://developer.apple.com/documentation/uikit/extending-your-app-s-background-execution-time
- https://developer.apple.com/documentation/xcode/configuring-background-execution-modes
- https://developer.apple.com/documentation/uikit/encrypting-your-app-s-files
- https://developer.apple.com/documentation/security/ksecattraccessibleafterfirstunlock
- https://developer.apple.com/documentation/security/access-control-lists
- https://developer.apple.com/documentation/webkit/wknavigationdelegate/webviewwebcontentprocessdidterminate(_:)
- https://developer.apple.com/documentation/foundation/nsextensioncontext/open(_:completionhandler:)
- https://developer.apple.com/documentation/uikit/uiapplication/openurloptionskey/openinplace
- https://developer.apple.com/documentation/bundleresources/information-property-list/lssupportsopeningdocumentsinplace
- https://developer.apple.com/library/archive/documentation/General/Conceptual/ExtensibilityPG/Share.html
- https://developer.apple.com/library/archive/qa/qa1587/_index.html

**Android / Google / Microsoft**

- https://developer.android.com/develop/background-work/background-tasks/persistent
- https://developer.android.com/reference/androidx/work/PeriodicWorkRequest
- https://developer.android.com/topic/performance/power/power-details
- https://developer.android.com/topic/performance/appstandby
- https://developer.android.com/training/monitoring-device-state/doze-standby
- https://developer.android.com/develop/background-work/services/fgs
- https://developer.android.com/develop/background-work/services/fgs/service-types
- https://developer.android.com/develop/background-work/services/fgs/timeout
- https://developer.android.com/develop/background-work/services/fgs/restrictions-bg-start
- https://developer.android.com/develop/background-work/services/fgs/changes
- https://developer.android.com/about/versions/15/behavior-changes-15
- https://developer.android.com/about/versions/16/behavior-changes-all
- https://developer.android.com/develop/background-work/services/alarms
- https://developer.android.com/develop/background-work/background-tasks/uidt
- https://developer.android.com/privacy-and-security/direct-boot
- https://developer.android.com/privacy-and-security/keystore
- https://developer.android.com/reference/androidx/security/crypto/EncryptedSharedPreferences
- https://developer.android.com/reference/android/security/keystore/KeyGenParameterSpec
- https://developer.android.com/identity/data/autobackup
- https://developer.android.com/guide/components/activities/process-lifecycle
- https://developer.android.com/training/app-links/verify-android-applinks
- https://firebase.google.com/docs/cloud-messaging/android-message-priority
- https://learn.microsoft.com/en-us/windows/win32/api/wincred/ns-wincred-credentiala

**Community plugin repos**

- https://github.com/Choochmeque/tauri-plugin-notifications
- https://github.com/yanqianglu/tauri-plugin-mobile-push
- https://github.com/srod/tauri-plugin-fcm
- https://github.com/SableClient/tauri-plugin-notifications
- https://github.com/HuakunShen/tauri-plugin-keyring
- https://github.com/s00d/tauri-plugin-keyring-store
- https://github.com/dardourimohamed/tauri-background-service
- https://github.com/tauri-apps/awesome-tauri

---

## 6. UNVERIFIED items

1. Minimum **Xcode** version required (changelogs reference Xcode 27 builds; no stated minimum found).
2. Per-target cross-compile success of `keyring` 4.2.0 for `aarch64-apple-ios` / `aarch64-linux-android` — source-level `cfg` gating is verified, but docs.rs flags "some builds failed" for 4.2.0.
3. The iOS cap on pending scheduled local notifications (commonly cited as 64) — no Apple doc page located.
4. iOS keychain persistence across app uninstall — widely reported and mechanistically explained, but no Apple documentation page found.
5. Exact sandbox location of the dialog plugin's iOS copy-on-pick (temp vs `Documents/Inbox`).
6. iOS **cold-start** `RunEvent::Opened` delivery under the scene lifecycle — tao ignores `launchOptions` and `ConnectionOptions.urlContexts`; warm start is implemented. Test on device.
7. iOS `downloadDir()` value (resolved through `dirs`).
8. Where cargo-mobile2's `additional-targets` / `ios-additional-targets` is declared (the template hook and the custom-template escape hatch are verified).
9. macOS/Windows runtime behaviour of the community `tauri-plugin-keyring-store` — claimed by its README and dependency graph only; not executed.
10. crates.io publish status of the SableClient and inKibra forks (both declare the colliding crate name `tauri-plugin-notifications`); their npm packages are verified.
11. The exact iOS version at which a force-quit app is never woken — widely repeated, not stated in the Apple pages read.
12. `kSecAttrAccessible…ThisDeviceOnly` non-migration semantics.

> Note: GitHub's REST API returned HTTP 403 (rate-limited, unauthenticated) during this run, so star counts, open-issue counts and some last-commit dates were taken from GitHub `.atom` feeds and HTML commit pages instead.
