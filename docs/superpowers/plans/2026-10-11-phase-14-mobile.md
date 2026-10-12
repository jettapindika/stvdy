# Phase 14. Mobile. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`.
> This plan implements them; it never redefines them. Where this plan and the
> roadmap disagree, the roadmap wins and this file is the bug.

**Goal:** the same SPA that ships on web and desktop also ships to the App Store and
Google Play, on a phone and a tablet, with a native layer thin enough to be understood
in one sitting. A push notification taps through to the right screen on a cold start, a
PDF shared from Files lands in a workspace, the sync engine runs from a native
scheduler when the app is not in the foreground, and the shipped bundle contains no
purchase path of any kind.

**Architecture:** the phone is the *same app* with a narrower window and a native shell
that does five things the plugin layer cannot do. The SPA is unchanged in kind — the
`useFormFactor` hook already renders the phone shell below 768px, the `Platform`
adapter already has `secureStore`, `notifications`, `files` and `deepLinks`, and the
sync engine already lives in Rust (`crates/sync/src/client/` over `rusqlite`, Phase 8).
Phase 14 adds (a) a pinned community push-receive plugin, (b) **one** custom mobile
plugin that owns both the iOS `BGTaskScheduler` / Android `WorkManager` background
seam *and* the Android share-intent intake, and (c) a hand-wired iOS **Share
Extension** target plus an App Group, because no plugin can add an app-extension target.
The purchase absence is proved by inspecting the *shipped artifact*, not by reading a
component.

**Tech Stack:** Rust (Tauri 2 shell, a new `src-tauri/plugins/mobile/` crate),
`rusqlite` + `reqwest` (Phase 8, already in the shell), Kotlin (Android `WorkManager`
worker + `FirebaseMessagingService` + share intent), Swift (iOS `BGTaskScheduler`
handlers + the Share Extension), React + Vite + TypeScript + Tailwind (unchanged),
`puppeteer-core` (dev-only, for the phone-viewport measurements), GitHub Actions macOS +
Android runners.

**Exit gate (observable, from the roadmap):** `tauri ios build` and `tauri android
build` both produce a bundle; **no purchase UI anywhere in the mobile bundle**;
push tokens register; deep links round-trip.

**How the second clause is proved, and what that changed.** "No purchase UI anywhere in
the mobile bundle" is a claim about *bytes*, so it needs a build that can satisfy it and a
check pointed at that build. Neither was true when the phase landed: the router registered
`billing/checkout/:paymentId` at run time, and the check read `app/dist` while that
directory held the web build — so five checkout strings sat in the one chunk every platform
installed and the check called it the mobile bundle. Both halves are now real:
`vite.config.ts` defines `__PURCHASE_UI__` (a build-time constant, so the purchase routes
and `POST /billing/checkout` are genuinely dropped rather than merely unreachable at run
time), and `scripts/check-no-purchase-ui.mjs` reads the artifact's own
`stvdy-build-target` stamp and **refuses to pass on anything but `store`**. Measured: the
store entry chunk holds zero occurrences of `billing/checkout` against five in the
web/desktop one. `docs/ARCHITECTURE.md` § Store-absence checks must validate the artifact
that actually ships states the rule and what neither half covers.

**Not established by any of that:** that `tauri ios build` / `tauri android build` run and
produce a bundle on this machine — that needs Xcode and the Android SDK, and it is the
roadmap clause's first half, not this one. The mechanism the store build depends on is
verified from the CLI's own source (it sets `TAURI_ENV_PLATFORM` for `beforeBuildCommand`
from the target triple's host component), and the store build is reproduced in CI with the
same variable, but no mobile bundle has been built here.

**Three roadmap gates are not optional here, and gate 5 is at its most literal:**
- Gate 5, accessibility: every measured item is checked **on a phone viewport
  (390×844)**, and the ≥44px interactive-target check is measured in a real browser,
  because jsdom has no layout and the existing `a11y.ts` says so.
- Gate 6, Indonesian end to end: an Indonesian workspace list, an Indonesian chat
  answer and an Indonesian reminder string are checked **on the phone shell**, because
  the phone is where a truncated translation or a placeholder-parity slip is most
  visible.
- Gate 7, honesty: the mobile plan screen shows the plan and the credits the app
  computes, never a price, and the "AI can make mistakes. Check the citations." hint
  sits under every composer on the phone too.

---

## This phase's real risk, and the defence

The roadmap names the risk itself: **running the SPA's JavaScript while the app is
backgrounded or terminated is impossible on both platforms**, and a store review
rejects a mobile bundle that offers an off-store purchase path.

The first half is architectural, not a bug to be worked around. On iOS a suspended app
runs no code and the `WKWebView` lives in a separate web-content process the OS may kill
at any time; on Android the process is *cached* and may receive "limited or no execution
time" (`docs/research/2026-10-11-tauri-2-capability-matrix.md` § 3.1). No plugin,
official or community, runs background JS. So **everything the product needs in the
background is native**: the sync engine is already Rust (Phase 8), the scheduler is a
native seam that calls it, and JS only observes and replays state when the app is
foregrounded. The mechanical defence is that the background entry points
(`BGAppRefreshTask`, `BGProcessingTask`, the `WorkManager` worker, the FCM service)
call the *Rust* `sync::sync_once` directly and never touch the webview — and a test
asserts the seam does not reference the frontend.

The second half is a policy fact with a proof obligation. The absence of purchase UI is
not a claim about a component; it is a property of the artifact the reviewer downloads.
The defence is `scripts/check-no-purchase-ui.mjs`, a build-inspection check in the exact
shape of the existing `scripts/check-no-cdn.mjs`: it reads every shipped file of the
**mobile** bundle and fails on a denylist of purchase markers (`Rp`, "QRIS", "Checkout",
"Upgrade to", `POST /billing/checkout`, a `plans.toml` price literal), and it fails if
`config/billing_policy.toml` shows any of `allow_purchase_ui` / `show_prices` /
`allow_purchase_cta_in_locked_messages` as `true` for `ios` or `android`. Reading the
component is not the proof; inspecting the bytes that ship is.

---

## Global Constraints (copied from the roadmap; every task includes these)

- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes.
  Prompts live in versioned modules, never inline. `tracing` structured logs with
  request ids; log latency for retrieval, LLM calls, sync pull/push, jobs, webhooks.
- **Server is the only source of truth.** Clients never call LLM, search, or Google
  APIs directly. All heavy work is server-side. A mobile client is a *client*: it holds
  a cache and an outbox and nothing more.
- **Offline writes are limited to:** tasks, events, flashcard reviews, quiz attempts,
  study sessions, settings. Everything else queues nothing. This is the same list on a
  phone as on the desktop, because it is the same sync engine.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default
  `Asia/Jakarta`). Local-date math only through `chrono-tz`.
- **`config/billing_policy.toml` defaults:** `allow_purchase_ui`, `show_prices`,
  `allow_purchase_cta_in_locked_messages` = `true` for web/macos/windows, `false` for
  ios/android. Mobile builds contain no checkout screen, no purchase buttons, no
  off-store purchase links or prices.
- **Quality gate:** `cargo fmt --check`, `cargo clippy --workspace --all-targets --
  -D warnings`, `cargo test --workspace`, the `src-tauri` trio (`just check-shell`),
  `npm --prefix app test`, `npm --prefix app run build`, `node
  scripts/check-no-cdn.mjs`, `node scripts/check-no-purchase-ui.mjs`, and — new this
  phase — `tauri ios build` / `tauri android build`.
- **UI language English**, but Indonesian documents and queries must work end to end
  (`simple` tsvector config + multilingual embeddings).
- **Docs updated in the same phase:** `docs/RELEASE.md` (store policy + the mobile test
  checklist), `docs/ARCHITECTURE.md` (§ Native shims chosen), `docs/ASSUMPTIONS.md`
  (A310+).

---

## What already exists (checked in this tree, not assumed)

**The SPA already has a phone shell and a platform adapter; Phase 14 wires native, not
layout.**
- `app/src/lib/useFormFactor.ts` — `formFactorForWidth(width)` returns
  `phone | tablet | desktop` at 768px / 1024px, and `useFormFactor()` reads it with
  `useSyncExternalStore` so the first render is correct. The doc comment already says
  the breakpoints must match the CSS (`md`/`lg`) and why it is *not* part of the
  platform adapter.
- `app/src/components/layout/Shell.tsx` renders `<Sidebar/>` above 768px and
  `<TabBar/>` below it (`md:hidden`), with `pt-safe-top` / `pb-safe-bottom` for the
  notch and home indicator. `TabBar.tsx` documents the 200%-text-scale `overflow-x-auto`
  fix and the 44px (`min-height: 2.75rem`) target.
- `app/src/platform/types.ts` — `Platform` already declares `secureStore`,
  `notifications` (`requestPermission`/`notify`/`schedule`), `files`
  (`pick`/`read`), `deepLinks` (`current`/`onOpen` with a classified
  `DeepLinkIntent`), `haptic`, `openExternal`, `checkForUpdate`.
- `app/src/platform/mobile.ts` — the iOS/Android adapter exists: notifications via
  `@tauri-apps/plugin-notification` (including `schedule`), haptics via
  `@tauri-apps/plugin-haptics`, `checkForUpdate()` returns `false` (stores own updates).
- `app/src/features/billing/policy.ts` — `parsePolicy` reads
  `config/billing_policy.toml` (imported `?raw`) and `lockedFeaturePolicy(kind)` returns
  `{ purchaseAllowed, showPrices, showUpgradeCta }`, defaulting to `DENY` on any parse
  failure.
- `app/src/components/ui/LockedFeature.tsx` — branches on `policy.showUpgradeCta`; the
  iOS/Android variant renders `t('lock.notOnPlan')` and **no link**.
- `app/src/router.tsx` — `/billing` is currently a `NotBuiltRoute` placeholder; there is
  no platform gate on it yet (Phase 13 builds the screen; Phase 14 gates it).

**The shell has a native layer and one generated mobile project, but no Android project
and no custom plugin.**
- `src-tauri/Cargo.toml` — `tauri 2.12.3`, `tauri-plugin-deep-link 2.6.1`,
  `tauri-plugin-notification 2.6.0`, `tauri-plugin-dialog 2.8.2`, `tauri-plugin-fs
  2.7.0`, `tauri-plugin-opener 2.7.1`, `tauri-plugin-os 2.4.0`; per-target keyring
  stores (`apple-native-keyring-store` `keychain` on macOS / `protected` on iOS,
  `windows-native-keyring-store`, `android-native-keyring-store`) over `keyring-core
  1.0.0`; `tauri-plugin-haptics 2.4.2` for `cfg(any(ios, android))`. **`tauri-plugin-updater`
  is deliberately absent** (needs the Phase 15 keypair).
- `src-tauri/src/lib.rs` — registers the plugins above, `#[cfg(desktop)]`-gates
  single-instance and global-shortcut, `#[cfg(any(ios, android))]`-gates haptics, and
  exposes `health_ping`, `deep_link_current`, `secret_{set,get,delete}`. The
  `mobile_entry_point` attribute is already present.
- `src-tauri/src/deeplink.rs` — the pure classifier (`auth_callback`, `auth_ticket`,
  `calendar`, `ignored`); Phase 1 verified the round trip on macOS (cold start and
  running instance).
- `src-tauri/capabilities/{default,desktop,mobile}.json` — `default` is cross-platform;
  `mobile.json` is `platforms: ["iOS", "android"]` with only `haptics:default`.
- `src-tauri/tauri.conf.json` — `plugins.deep-link.mobile` is `[{ "scheme": ["stvdy"] }]`;
  there is **no `bundle.iOS` or `bundle.android` block**, so the defaults apply
  (`minimumSystemVersion 15.0`, `minSdkVersion 24`).
- `src-tauri/gen/apple/` **exists** (`project.yml`, `stvdy-shell_iOS/Info.plist`,
  entitlements stub, `Podfile`) — `tauri ios init` ran in Phase 1. `src-tauri/gen/android/`
  **does not exist**: `tauri android init` has never run.

**The server side of push already exists.**
- `migrations/0004_devices_auth_push.sql` — `push_tokens (device_id uuid primary key
  references devices on delete cascade, provider text, token text, updated_at)`. The
  table is keyed on the device, so a re-registration upserts rather than double-writes.
- `crates/server/src/devices.rs:34` — `PUT /devices/{id}/push-token` (`put_push_token`),
  with `PUSH_PROVIDERS = ["apns", "fcm"]` rejecting any other provider
  (`devices.rs:27-28`). **The client's only job is to obtain a token and PUT it.**

**The absence-check pattern already exists.**
- `scripts/check-no-cdn.mjs` — reads every shipped file under `app/dist`, matches the
  forms that cause a *load* (never a bare URL in a string), and fails with a readable
  list. `scripts/check-no-purchase-ui.mjs` is the same shape against a denylist.
- `app/src/test/a11y.ts` documents that **jsdom cannot evaluate layout**, which is why
  contrast lives in `src/tokens/contrast.test.ts` and the 44px check cannot be a vitest
  assertion.

**MISSING (what this phase adds):** `bundle.iOS`/`bundle.android` config; `tauri android
init`; the push-receive plugin and its `google-services.json`/APNs wiring; the custom
mobile plugin (`src-tauri/plugins/mobile/`) with the background scheduler and Android
share intake; the iOS Share Extension target + App Group; the SPA's push-token
registration; the `/billing` platform gate; `scripts/check-no-purchase-ui.mjs` and
`scripts/measure-mobile.mjs`; the mobile CI job; the mobile test checklist and the
store-policy note's mobile specifics in `docs/RELEASE.md`.

---

## Inherited from Phases 8–13 (hard dependencies — not re-planned here)

Phases 8–13 land first. Phase 14 **consumes** their output and re-plans none of it:

- **Phase 8 hands over the entire mobile data layer.** `crates/sync/src/client/`
  (`Engine`, `sync_once`, `status`, `LocalStore`, `Transport`, the outbox, the conflict
  table) plus `src-tauri/src/{cache,sync}/` (the `rusqlite` connection, the schema, the
  `reqwest` transport, the IPC surface) and `src-tauri/tests/cache.rs`. This *is* the
  bulk of Phase 14's data work, and the background scheduler is a thin native caller of
  `sync::sync_once`. **Do not re-plan the engine.**
- **Phase 9** adds `local_state_get`/`local_state_set` over the same SQLite file (the
  focus timer) and `app/src/platform/{web,desktop,mobile}.ts` gains `localState`.
- **Phase 12** owns the *server* push send path (`PushProvider`, the `send_push` job
  handler, the `PushMessage` shape) and the reminder engine. Phase 14 owns only the
  *client* receive path (token acquisition, registration, tap-to-open).
- **Phase 13** builds the billing/pricing screens for web/desktop and the
  `billing_policy` server enforcement (`POST /billing/checkout` refused for
  `platform in (ios, android)`, `GET /plans` omits prices there). Phase 14 gates the
  SPA's `/billing` route and proves the absence in the bundle.

The **`/billing` route**: Phase 13 leaves the web/desktop screen; Phase 14 wraps it so
that on `platform.kind() in ('ios','android')` the route renders the read-only plan
screen (Task 9) instead. The gate is a route element chosen by `platform.kind()`, not a
flag inside the screen, so the checkout component is never in the mobile chunk.

---

## Decisions this plan pins (so parallel work cannot diverge)

1. **A310 — Push receive is a pinned community plugin, with a vendored fallback path.**
   There is no official APNs/FCM receive plugin, and the official
   `tauri-plugin-notification` **provably cannot receive remote push** (its iOS delegate
   skips `UNPushNotificationTrigger`; its Android manifest registers no
   `com.google.firebase.MESSAGING_EVENT` service). Phase 14 pins **`tauri-plugin-notifications`
   (Choochmeque) 0.5.0-rc.14** — APNs + FCM, no custom Swift/Kotlin, active — and records
   the fallback in `docs/ARCHITECTURE.md`: if it regresses before submission, the same
   task's seam (a `PushToken` command + an `onNotification` event) is served by a custom
   plugin, which the capability matrix budgets for regardless.
2. **A311 — The background scheduler and Android share intake are ONE custom plugin,
   `src-tauri/plugins/mobile/`.** The capability matrix's "one native-work package" is
   split only where the *platform mechanism* differs: the iOS Share Extension is an
   app-extension **target** and cannot be a plugin (A312), so it is hand-wired; push is
   a crate (A310); everything else native — `BGTaskScheduler` registration, the
   `WorkManager` worker, the FCM service seam, the Android `ACTION_SEND`/`ACTION_VIEW`
   intake — is this one plugin, because they share a `setup()` hook and a Rust entry
   point. Three separate plugins would be three copies of the same lifecycle code.
3. **A312 — The iOS Share Extension is a hand-wired target plus an App Group, added via
   `bundle.iOS.template`.** A Share Extension cannot launch its host app
   (`NSExtensionContext.open(_:)` is supported only by the Today and iMessage extension
   points), so the extension writes the shared file/URL into an **App Group** shared
   container and the main app imports it on next foreground. Tauri has no `bundle` key
   for an app-extension target, so the target is added by rendering a custom XcodeGen
   `project.yml` (`bundle.iOS.template`, confirmed present in the schema) over the
   generated one. The App Group identifier is `group.app.stvdy` and is the *only*
   shared-container name in the tree.
4. **A313 — File intake has three channels, and each platform uses the ones it has.**
   (a) iOS "Open in…" document associations (`bundle.fileAssociations[]` →
   `CFBundleDocumentTypes`) deliver via `RunEvent::Opened`; (b) the iOS Share Extension
   (A312) writes to the App Group; (c) Android `bundle.fileAssociations[]` with a
   **concrete** `mimeType` injects `SEND`/`VIEW` intent filters and delivers a
   `content://` URI. A share cannot be expressed as a deep link, so a deep-link-only
   "share" flow is not built.
5. **A314 — An Android picked or shared file is copied into app storage immediately.**
   `ACTION_GET_CONTENT` grants are transient and the dialog plugin never calls
   `takePersistableUriPermission`, so the grant dies when the receiving activity
   finishes. The intake reads the `content://` URI with the URL-based fs API and copies
   it before returning; the copied path is what the upload queue holds.
6. **A315 — The background sync seam calls Rust, never the webview.** `BGAppRefreshTask`
   (≤30s) and `BGProcessingTask` on iOS, one unique periodic `WorkManager` job (≥15 min)
   on Android, each call `stvdy_shell_lib::sync::sync_once` directly. JS observes the
   result on the next foreground via the Phase 8 sync-status surface. A test asserts the
   seam module does not reference `tauri::WebviewWindow` or emit a frontend event.
7. **A316 — Push delivery is treated as an opportunistic hint, never a schedule.** iOS
   does not guarantee silent-push delivery, throttles it to roughly 2–3 per hour, keeps
   only the newest held notification and discards it on force-quit; Android defers work
   in Doze and caps standby-bucket quotas. So the app **always re-syncs on foreground**
   and never relies on a background execution window for correctness. Tap-to-open
   (including cold-start buffering) is the only guaranteed JS surface.
8. **A317 — The scheduler is best-effort and its inaccuracy is documented, not hidden.**
   `BGTaskScheduler` scheduling is discretionary ("the system decides the best time");
   Android periodic work is inexact and ≥15 min. The mobile test checklist states what
   degrades to a simulator (everything except APNs/FCM token registration and the Share
   Extension row, which need a physical device) and what the user sees when a background
   window never fires (the next foreground sync, with the honest "Last synced …" line
   Phase 8 already renders).
9. **A318 — `SCHEDULE_EXACT_ALARM` is added only if a reminder needs minute accuracy,
   and the design assumes it is not granted.** The official notification plugin falls
   back to inexact `setAndAllowWhileIdle` on API 31+ and declares no
   `SCHEDULE_EXACT_ALARM`. Phase 14 declares it in the mobile plugin's manifest (it is
   user-grantable and revocable) but designs every reminder so inexact delivery is
   acceptable, because a reminder that must be exact is a reminder that is wrong on a
   phone that denies the permission.
10. **A319 — Secure storage is the Phase 1 keyring path, verified on device, with an
    Android backup-exclusion note.** The per-platform keyring stores are already wired
    (`src-tauri/Cargo.toml`) and the `secret_*` commands exist. Phase 14 verifies the
    round trip on a device and records the one caveat the matrix names: Android Auto
    Backup includes the app dir by default and a restored install loses Keystore keys, so
    the token must be re-acquired after a restore (a re-login, which the app already
    handles). No new secure-storage code.
11. **A320 — Universal links / app links reuse the existing classifier and are
    config-only.** `bundle.iOS.associatedDomains` (`applinks:app.stvdy`) and Android
    `autoVerify` intent filters (tauri-build generates them) point at
    `https://app.stvdy/.well-known/apple-app-site-association` and
    `https://app.stvdy/.well-known/assetlinks.json`; both feed the same
    `deeplink::classify` path, so no second URL parser exists.
12. **A321 — The no-purchase-UI check inspects the shipped bundle, and it is a CI job.**
    `scripts/check-no-purchase-ui.mjs` reads every file of the store build and fails on
    a denylist plus a `billing_policy.toml` re-assertion. It runs in CI immediately after
    the web build, because a check that is only run by hand is the one that is skipped.
    **It also refuses to pass on an artifact that is not the store build**, which is the
    half that makes the rest mean anything: every build stamps its `index.html` with
    `<meta name="stvdy-build-target" content="store"|"full">`, and a `full` artifact — or
    one with no stamp — is an offender. That gate exists because the check was written
    through the opposite state: it read `app/dist` while `app/dist` held the *web* build,
    whose bytes legitimately contain the checkout route, and a green result would have
    been read as "the phone build sells nothing". `scripts/check-no-purchase-ui.test.mjs`
    pins both directions — a clean `full` artifact must fail, an unstamped one must fail,
    and a `store` stamp must not excuse a purchase path in the bytes.
13. **A322 — The read-only plan screen is the mobile `/billing` route, and no purchase
    route exists in the mobile router.** On `ios`/`android` the route renders a screen
    that shows the plan name, credits remaining and the usage the app computes, with the
    neutral `lock.notOnPlan` copy — never a price, never a link. **The checkout component
    is excluded from the store build at build time, not at run time**, and the distinction
    is the whole mechanism: `vite.config.ts` defines `__PURCHASE_UI__` (`false` in a store
    build), `router.tsx` spreads `/pricing` and `/billing/checkout/:paymentId` into the
    route children behind it, and `PlanUsageScreen` guards `BillingScreen` the same way. A
    `define` is substituted before the minifier runs, so the branch is provably dead, those
    screens lose every remaining reference, and `api/queries/billing.ts` — which holds
    `POST /billing/checkout` — is dropped with them. The original form of this entry said
    the component was "tree-shaken out because the route element is chosen at the router",
    and that was false: `billingElementFor` reads `platform.kind()` at **run** time, which
    keeps both route elements in one chunk. Runtime routing and build-time exclusion are
    different mechanisms, and only the second removes bytes. Which builds are store builds
    is decided by the shell: `tauri build` sets `TAURI_ENV_PLATFORM` to the target it is
    packaging (the host component of the target triple — `ios` for `aarch64-apple-ios`,
    `android` for `aarch64-linux-android`), so the `frontendDist` directory
    (`../app/dist`) holds the phone bundle exactly when the shell packages for a phone.
    `VITE_PLATFORM=mobile` makes the same build into `dist-mobile` without a shell.
    **Measured:** the store entry chunk contains zero occurrences of `billing/checkout`
    where the web/desktop one contains five.
14. **A323 — The 44px target check is measured in real headless Chrome at 390×844.**
    `scripts/measure-mobile.mjs` (`puppeteer-core`, `CHROME_PATH` or the same probe list
    as `crates/server/src/render/chrome.rs`) loads the running app at a phone viewport,
    measures every interactive element's `getBoundingClientRect()` (≥44×44 CSS px),
    checks contrast on the default dark and light themes from computed colours, and
    re-measures at 200% text scale for clipping/overlap. jsdom cannot do this, which the
    repo's own `a11y.ts` says.
    **Its coverage is part of the gate, and it is stated rather than left implicit**
    (Phase 14's Task B). `scripts/measure-mobile.mjs` walks **13 surfaces**
    at 390×844, both themes, 100% and 200% text: `today`, `workspaces`, `calendar`,
    `tasks`, `settings`, `plan`, `workspace-sources`, `workspace-chat`,
    `workspace-studio-flashcards`, `workspace-studio-quiz`, `workspace-studio-report`,
    `dashboard` and `workspace-chat-proposal`. **Not measured, with the reason:**
    `/devices`, `/study/:deckId`, `/quiz/:quizId` and `/reports|cheatsheets/:id` are
    reachable by tap and one entry away, but were outside Task B's named minimum;
    `/planner`, `/syllabus` and `/syllabus/:draftId` are **not reachable from the tab
    bar at all** (the only in-app link to `/planner` is inside a syllabus draft), so
    they need a click path the walk does not have and Phase 11 drives that leg itself;
    `/pricing`, `/billing` and `/billing/checkout/:paymentId` are Phase 13's gate and
    run through this harness; the dashboard's weak-topic list renders `LockedFeature`
    on the fixture's `free` account by design; the auth routes sit outside the
    signed-in shell; `/styleguide` is linked from nowhere; and nothing native is
    verifiable on this machine (no Android SDK, no signing identity, no device).
    The walk is **phone-width by default** — `VIEWPORT_WIDTH`/`VIEWPORT_HEIGHT`/
    `VIEWPORT_DPR`/`SCALES`/`OUTPUT` override it so gate 5's desktop leg uses this
    harness rather than a second copy of it.
    **The desktop leg did not actually work until `bc7eed8`, and the failure was
    silent until someone ran it:** a `nav` step names the phone tab bar's label, and
    the tab bar is `md:hidden` with "Work"/"More" while the desktop sidebar is
    `hidden md:flex` with "Workspaces"/"Settings", so at 1365px the walk died on
    `FAIL: no nav a labelled "Work"` after the first surface. `bc7eed8` added a
    one-step label alias and the leg now completes (80 combinations at 1365×900).
    The first real desktop run found three things, none of them the rem class and
    none of them fixed here: the sidebar's nav items measure **215×40** — under 44px
    in height at 100% text, both themes, on every surface; `button.cite` is 36.4×15.2
    because its 44px rule sits inside `@media (max-width: 767px)` (deliberate,
    recorded so nobody "fixes" it); and two `p.text-xs` paragraphs in the workspace
    panels overflow their box at 200% text, inside an `overflow-y-auto` panel whose
    `overflow-x` computes to `auto`, in a grid track declared `minmax(200px, 1fr)` —
    a **px** floor that does not grow with the text, so the panel scrolls sideways.
    That last one is *reachable* by scrolling, which is why the `clipped` check was
    left alone: extending `inScroller` to it would also have exempted the
    `SourcePicker` rows (inside `div.max-h-32.overflow-y-auto`, `overflow-x` also
    computed `auto`) that were clipping a source's title.
    **The class is not limited to text-scale inflation, and a walk that measured only
    at 200% would have missed the most valuable defect this series found.** The
    dashboard's overflow measured `423 > 390` **at 100% text** — the first defect in
    the whole series visible at normal size, and a different mechanism from the
    rem-doubling one: a `truncate`'s `white-space: nowrap` min-content propagating up
    through a flex column until an *ancestor* reports the overflow, where the element
    that is too wide reports no overflow of its own. Both scales are therefore
    measured, and both are part of the pass condition.
15. **A324 — The adaptive UI is width-based, and Phase 14 verifies it rather than
    introducing a second convention.** `useFormFactor` (768/1024px) is the only
    form-factor source. Phase 14 adds a `phone`/`tablet` render test and the measured
    viewport check; it does not add a platform-keyed layout.
16. **A325 — Mobile updates ship through the stores; the updater stays desktop-only.**
    `tauri-plugin-updater` marks iOS/Android `none` and is already absent from the
    shell's dependencies. `docs/RELEASE.md` records that TestFlight/Play is the mobile
    update channel and the desktop updater is Phase 15's.
17. **A326 — TestFlight and Play internal testing are the distribution channels, and
    the mobile test checklist is a document, not a claim.** `docs/RELEASE.md` gains the
    checklist (device builds, both platforms; push token; deep-link round trip on cold
    start and warm; share import; background sync fires or the foreground fallback is
    honest; the bundle inspection passes; the gates run on a phone viewport) with a
    pass/fail column filled at submission time.
18. **A327 — The mobile CI job is macOS for iOS and ubuntu for Android, and a device
    build is a prerequisite, not a CI step.** CI proves the projects *build*; it cannot
    prove APNs/FCM registration or the share-sheet row, which need a physical device and
    a store account. Those are named as user-side prerequisites in `docs/RELEASE.md` and
    the exit checklist, not silently assumed.
19. **A328 — `google-services.json` and the APNs key are secrets, not committed files.**
    The Android Google Services plugin needs `google-services.json`; iOS needs an
    `aps-environment` entitlement and a signing identity. Both are provided at build time
    from CI secrets (or a local untracked file named by an env var), and the repo ships a
    `google-services.json.example` documenting the shape. A committed service file would
    be a leaked project credential.
20. **A329 — The push token is registered against the device row created at login.**
    `PUT /devices/{id}/push-token` (`devices.rs:34`) already exists and upserts on
    `device_id`. The client calls it after login and on every token refresh, sending
    `provider = "apns" | "fcm"`; no new server route is added, and `push_tokens` is not a
    `+sync` table (a token is per-device and server-owned), so `0017_sync_triggers.sql` is
    untouched.

---

## File structure (new this phase)

```
src-tauri/tauri.conf.json                  # + bundle.iOS / bundle.android / associatedDomains
src-tauri/tauri.conf.json                  # + plugins.notifications (push)
src-tauri/Cargo.toml                       # + tauri-plugin-notifications, + plugins/mobile
src-tauri/capabilities/mobile.json         # + the push + mobile-plugin permissions
src-tauri/src/lib.rs                       # + the notifications plugin, + mobile plugin init
src-tauri/src/push.rs                      # NEW — the push-token command + registration helper
src-tauri/plugins/mobile/Cargo.toml        # NEW — the custom mobile plugin crate
src-tauri/plugins/mobile/build.rs          # NEW
src-tauri/plugins/mobile/src/lib.rs        # NEW — Builder, init, the commands
src-tauri/plugins/mobile/src/scheduler.rs  # NEW — the Rust seam (calls sync::sync_once)
src-tauri/plugins/mobile/android/          # NEW — Kotlin: WorkManager worker, FCM service, share intent
src-tauri/plugins/mobile/android/build.gradle.kts
src-tauri/plugins/mobile/android/src/main/AndroidManifest.xml
src-tauri/plugins/mobile/android/src/main/java/app/stvdy/mobile/*.kt
src-tauri/plugins/mobile/ios/Sources/*.swift   # NEW — BGTaskScheduler registration + handlers
src-tauri/gen/apple/project.yml.template   # NEW — the custom XcodeGen template (Share Extension)
src-tauri/gen/apple/ShareExtension/        # NEW — the Share Extension target's Swift + Info.plist
src-tauri/gen/android/                     # NEW — generated by `tauri android init`
src-tauri/google-services.json.example     # NEW — the shape, not the credential
app/src/platform/types.ts                  # + PushApi (token, onNotification)
app/src/platform/mobile.ts                 # + the push receive path
app/src/features/notifications/            # NEW — register on login, tap-to-open routing
app/src/features/billing/MobilePlanScreen.tsx  # NEW — the read-only plan screen
app/src/router.tsx                         # + the platform-chosen /billing element
scripts/check-no-purchase-ui.mjs           # NEW — the bundle inspection
scripts/measure-mobile.mjs                 # NEW — the phone-viewport measurements
scripts/smoke-phase14.sh                   # NEW
app/package.json                           # + puppeteer-core (devDependency)
.github/workflows/ci.yml                   # + the `mobile` job (iOS + Android build)
docs/RELEASE.md                            # + the store-policy mobile specifics + the checklist
docs/ARCHITECTURE.md                       # + § Native shims chosen (Phase 14)
docs/ASSUMPTIONS.md                        # + A310–A329
```

---

## Task 1: The mobile bundle config, both generated projects, and the build commands

**Concern:** get `tauri ios build` and `tauri android build` to run at all, with the
right minimum versions and no purchase surface in the bundle metadata.

**Files:**
- Modify: `src-tauri/tauri.conf.json`
- New: `src-tauri/gen/apple/project.yml.template` (the custom XcodeGen template the config
  names — it must exist *before* `ios init`, see Step 3), `src-tauri/gen/apple/Info.ios.plist`
  (the plist input the CLI merges, likewise), `src-tauri/tests/config.rs`
- New (generated): `src-tauri/gen/android/` (via `tauri android init`)
- Modify: `.github/workflows/ci.yml` — **owned by another slice; not edited here**

**Interfaces:**
- Consumes: the Tauri CLI 2.12.2 (`app/node_modules/.bin/tauri`).
- Produces: `bundle.iOS` / `bundle.android` config; a buildable iOS and Android project.

- [x] **Step 1: Write the failing test — the config must declare the mobile bundle**

Add `crates/server/tests/mobile_config.rs`? No — the shell is a separate workspace and
the server cannot see `tauri.conf.json` as a typed value. The failing test is a **JSON
shape assertion in the shell's own test file**, `src-tauri/tests/config.rs` (new):

```rust
//! The mobile bundle config, asserted as data.
//!
//! A missing `bundle.android` block does not fail a build — it silently takes the
//! defaults — so the values that matter to a store submission are pinned here.

#[test]
fn the_mobile_bundle_declares_its_floor_and_its_deep_link_domain() {
    let raw = include_str!("../../tauri.conf.json");
    let config: serde_json::Value = serde_json::from_str(raw).expect("tauri.conf.json parses");
    let bundle = &config["bundle"];
    assert_eq!(bundle["iOS"]["minimumSystemVersion"], "15.0");
    assert_eq!(bundle["android"]["minSdkVersion"], 24);
    assert_eq!(
        bundle["iOS"]["infoPlist"].as_str(),
        Some("gen/apple/Info.ios.plist"),
        "the merged plist is where the App Group and the background modes are declared"
    );
}
```

- [x] **Step 2: Run it and watch it fail**

`cargo test --manifest-path src-tauri/Cargo.toml --test config` →
`assertion failed: `(left == right)`` — `bundle["iOS"]` is `Null`.

**Measured, against the pre-change `src-tauri/tauri.conf.json`:** the only edit needed to
watch it fail was removing the added `"iOS": { … }` block (i.e. reverting
`tauri.conf.json` to its state before this task). Result: `1 failed`, panicking at
`tests/config.rs:16` with `left: Null, right: "15.0"` — the plan's predicted failure
exactly. One deviation from the plan's snippet: the path is `../tauri.conf.json`, not
`../../tauri.conf.json`, because `include_str!` resolves relative to the file
(`src-tauri/tests/config.rs` → `src-tauri/tauri.conf.json`); `../../` would point at the
repo root, where there is no `tauri.conf.json`.

- [x] **Step 3: Add the config and generate the Android project**

In `src-tauri/tauri.conf.json`, add to `bundle`:

```jsonc
"iOS": {
  "minimumSystemVersion": "15.0",
  "developmentTeam": "",
  "infoPlist": "gen/apple/Info.ios.plist",
  "template": "gen/apple/project.yml.template"
},
"android": { "minSdkVersion": 24, "versionCode": 1 },
"fileAssociations": [
  { "ext": ["pdf"], "mimeType": "application/pdf", "name": "PDF" },
  { "ext": ["epub"], "mimeType": "application/epub+zip", "name": "EPUB" },
  { "ext": ["docx"], "mimeType": "application/vnd.openxmlformats-officedocument.wordprocessingml.document", "name": "DOCX" }
]
```

The `mimeType` must be **concrete** — the matrix verified `"*/*"` never matches. Then:

```
./app/node_modules/.bin/tauri android init --ci
./app/node_modules/.bin/tauri ios init
```

`tauri android init` writes `src-tauri/gen/android/` and installs the NDK
(`29.0.13846066`) via `sdkmanager`. **Prerequisite: an Android SDK at
`~/Library/Android/sdk`** (the Phase 1 spike recorded it absent). If the SDK is missing,
`android init` cannot run; the iOS half still does, and the Android half is the recorded
prerequisite.

**Result, measured on this machine.** `~/Library/Android/sdk` does not exist and
`ANDROID_HOME` is unset, so **`tauri android init` was not run** and
`src-tauri/gen/android/` does not exist. This is the prerequisite the paragraph above
already names, now recorded as an outcome rather than a possibility: the Android half of
this task (the generated project, `tauri android build --debug`, and the NDK install) is
**blocked on an Android SDK**, which was not installed (installing one is out of scope).

**Three deviations from the config block above, each forced by the CLI, each measured.**

1. **`"developmentTeam": ""` had to be dropped.** An empty string is not "unset": the CLI
   refuses to build the Apple config at all —
   `failed to create Apple configuration: `apple.development-team` is empty` (exit 1) —
   before it does anything else, for `ios init` and `ios build` alike. The field's schema
   is `["string", "null"]` and the template only emits `DEVELOPMENT_TEAM` under
   `{{#if apple.development-team}}`, so *absent* is the correct spelling of "no team
   configured"; a signing identity is a documented device-only prerequisite (A328) and is
   supplied through `APPLE_DEVELOPMENT_TEAM` at build time, which is what the field's own
   description says.
2. **`bundle.iOS.template` must name a file that already exists**, and the path is
   resolved **relative to the process's current directory**, not to `tauri.conf.json`.
   Running `tauri ios init` from the repo root fails with
   `failed to read custom Xcode project template gen/apple/project.yml.template: No such
   file or directory` even when `src-tauri/gen/apple/project.yml.template` is present —
   the CLI `set_current_dir`s to `src-tauri` only in `ios build`, not in `ios init`. Both
   commands therefore work from `src-tauri/`, which is how they were run. The file itself
   is the generated `project.yml` (Task 4 extends it), seeded by copying the generated
   one, exactly as the plan's Task 4 describes it.
3. **`bundle.iOS.infoPlist` names an *input* plist the CLI reads and merges**, not the
   merged output. `ios build` does
   `merge_plist([<scheme>/Info.plist, …version…, [Info.plist], [Info.ios.plist],
   bundle.ios.infoPlist, fileAssociations])` and writes the result back to
   `<scheme>/Info.plist`, so the named file must exist *before* the first build:
   `failed to parse plist from gen/apple/Info.ios.plist: Io(NotFound)`, exit 1. It is
   created here (an empty `<dict/>`, with a comment saying what Task 4 adds to it) because
   the config names it; Task 4 fills it with `UIBackgroundModes` and
   `BGTaskSchedulerPermittedIdentifiers`. The plan's own comment on the assertion — "the
   merged plist is where the App Group and the background modes are declared" — describes
   the merged *result*, and this file is the input that reaches it.

**One real defect in the generated project, found by building it and fixed here.** The
generated `project.yml`'s "Build Rust Code" phase runs `node tauri ios xcode-script …`,
and `node` resolves `tauri` as a *module* relative to the process's cwd, which for an
Xcode build phase is `src-tauri/gen/apple`. No `tauri` module exists there (the CLI lives
at `app/node_modules/@tauri-apps/cli/`), so the phase died with
`Error: Cannot find module '…/src-tauri/gen/apple/tauri'` and `xcodebuild` exited 65 —
`BUILD FAILED`. **A generated project that does not build is the finding**, so the fix is
in the template this task owns: `project.yml.template` addresses the CLI by path
(`node ../../../app/node_modules/@tauri-apps/cli/tauri.js ios xcode-script …`), and
`project.yml` is regenerated from it. This is a defect in `tauri-cli`'s own iOS template
as invoked from a project whose CLI is an npm dependency; it is recorded here because the
next `tauri ios init` that does *not* render our template would reintroduce it.

- [x] **Step 4: Run both builds**

```
./app/node_modules/.bin/tauri ios build --debug --target aarch64-sim   # needs an iOS simulator runtime
./app/node_modules/.bin/tauri android build --debug                    # needs the SDK + NDK
```

Both must exit 0. **Simulator degradation:** the iOS `aarch64-sim` build needs an
installed simulator runtime (the Phase 1 spike found `xcrun simctl list runtimes` empty);
a **device** build needs a signing identity. The Android debug build needs no device.

**Result, measured.** `tauri android build --debug` was **not run** — no Android SDK (Step
3). The iOS build **succeeds and produces an artifact**, and it took two fixes and one
machine-level precondition to get there. The whole sequence, in order, with real exit
codes:

| # | Command | Exit | First error |
|---|---|---|---|
| 1 | `tauri ios build --debug --target aarch64-sim` | 1 | `failed to parse plist from gen/apple/Info.ios.plist: Io(Os { code: 2 … NotFound })` — the input plist did not exist (Step 3, deviation 3) |
| 2 | same, after creating `Info.ios.plist` | 1 | `iOS platform not installed` — `ensure_ios_runtime_installed()` gates on `xcrun simctl list runtimes` finding a runtime whose name starts with `iOS`, and **none is installed** |
| 3 | same, with `xcrun` shimmed (see below) | 1 | `Error: Cannot find module '…/src-tauri/gen/apple/tauri'` → `** BUILD FAILED **`, `xcodebuild` exited 65 — the template defect (Step 3, above) |
| 4 | same, after fixing `project.yml.template` and regenerating | **0** | `** BUILD SUCCEEDED **` |

**Artifact:** `src-tauri/gen/apple/build/arm64-sim/Stvdy.app` — 104 MB, containing a
`Mach-O 64-bit executable arm64` named `Stvdy`. It is inside `gen/apple/build/`, which
`gen/apple/.gitignore` excludes, so it is a build product and not a committed file.

**The one honest caveat on run 4.** It was run with `/tmp/fakebin` prepended to `PATH`,
holding a three-line `xcrun` shim that answers `xcrun simctl list runtimes --json` with a
single synthetic `iOS 27.0` runtime and delegates every other invocation to the real
`xcrun`. **Nothing in the repository was changed by the shim** and it is not committed.
It was needed because run 2 shows the CLI's own precondition is not satisfiable here, and
the question this task actually has to answer — *does the generated project build?* — is
only reachable past it. The plan's note ("needs an iOS simulator runtime") is therefore
**literally true of the CLI as shipped**: the check is on a *runtime*, not on the SDK, and
the SDK being present (`iphonesimulator27.0`) is not enough. The shim changed nothing
about what was compiled; the compile, the Xcode build, the linking and the bundle are all
real, against the real `iphonesimulator27.0` SDK.

**Consequences for the exit checklist.** Row 4 ("iOS build … exit 0, bundle produced") is
**met on this machine only with the runtime precondition supplied**; as the plan's own
text says, the honest fix is `xcodebuild -downloadPlatform iOS`, which was not run because
it is a multi-gigabyte download and the disk had ~9 GB free after the build. Row 5
("Android build") is **not met**: no SDK. The `simulator` half of A317 stands as written.

**Disk.** The `aarch64-apple-ios-sim` compile grew `src-tauri/target` from 11 GB to 15 GB
and free space on `/` from 12 GB to 8.6 GB. `cargo clean` was not run. This is worth
knowing before the next mobile build on this machine.

- [x] **Step 5: Add the CI job**

`.github/workflows/ci.yml` gains a `mobile` job: a `macos-15` leg running
`tauri ios build --debug --target aarch64-sim` (with `xcodebuild -downloadPlatform iOS`
if the runner lacks the runtime) and an `ubuntu-24.04` leg running
`tauri android build --debug` (with `actions/setup-java`, the Android SDK, and the
pinned NDK). Both legs upload the bundle as an artifact so the exit checklist can quote
the command.

**Not done here, deliberately: `.github/workflows/ci.yml` is another slice's file.** A
parallel slice is adding a `mobile` job to it, and this task's file list claiming
`ci.yml` was written before that was known. Editing it from here would be a collision, so
the file was left untouched and this step is ticked as *owned elsewhere* rather than
*performed*. What the job needs is now known and is worth writing down for whoever adds
it: the `macos-15` leg **must** run `xcodebuild -downloadPlatform iOS` before
`tauri ios build` — Step 4 run 2 shows an `iphonesimulator` SDK without a *runtime* is not
enough and the failure is `iOS platform not installed`, not a compile error. The leg also
needs `npm ci --prefix app`, because the generated project's Rust build phase invokes the
CLI out of `app/node_modules`.

- [x] **Step 6: Commit**

```
git add -A && git commit -m "Phase 14 Task 1: mobile bundle config + both generated projects"
```

**The `git add -A` above was NOT used.** The repository's index has been misused five
times in one day by exactly that class of command and other slices have uncommitted files
in the tree right now (`app/**`, `crates/**`, `migrations/**`,
`.github/workflows/release.yml` are all dirty). The commit was made with an explicit path
list and `git diff --cached --stat` was checked before it. `git add -A` in a shared
working tree stages other people's work; the instruction is a bug in this plan and should
not be copied by the remaining tasks either.

---

## Task 2: The push plugin, pinned, and the receive path behind a seam

**Concern:** receive APNs/FCM, hand the token to Rust, and route a tap to the right
screen — with the plugin choice isolated behind one seam so a fallback is a swap.

**Files:**
- Modify: `src-tauri/Cargo.toml`, `src-tauri/src/lib.rs`, `src-tauri/capabilities/mobile.json`
- Modify: `src-tauri/tauri.conf.json` (the plugin's config block)
- New: `src-tauri/src/push.rs`
- New: `src-tauri/google-services.json.example`
- Test: `src-tauri/tests/push.rs`

**Interfaces:**
- Consumes: `tauri-plugin-notifications` 0.5.0-rc.14; `platform.kind()`.
- Produces: `push::register_token(app) -> Result<String, PushError>`,
  `push::provider_for(kind) -> &'static str` (`"apns"` | `"fcm"`).

- [x] **Step 1: Write the failing test**

`src-tauri/tests/push.rs`:

```rust
//! The push seam: the provider string and the token validation are pure and are
//! asserted here; the plugin's own registration is a device-only path.

#[test]
fn the_provider_string_matches_what_the_server_accepts() {
    // devices.rs:29 PUSH_PROVIDERS = ["apns", "fcm"]. A third string would be a
    // typo that sits in push_tokens and never delivers.
    assert_eq!(stvdy_shell_lib::push::provider_for("ios"), Some("apns"));
    assert_eq!(stvdy_shell_lib::push::provider_for("android"), Some("fcm"));
    assert_eq!(stvdy_shell_lib::push::provider_for("macos"), None);
}

#[test]
fn an_empty_token_is_refused_before_it_reaches_the_network() {
    assert!(stvdy_shell_lib::push::validate_token("").is_err());
    assert!(stvdy_shell_lib::push::validate_token("abc").is_ok());
}
```

Two cases were added to it, both because the plan's two cases leave the seam's actual
failure modes unchecked: a **whitespace-only** token (the server's own rule is
`token.trim().is_empty()`, so `"   "` is a `PUT` that can only 422) and a
**cross-check of the two provider lists** — `provider_for`'s answers are compared against
`PUSH_PROVIDERS` as it is written in `crates/server/src/devices.rs`, in both directions,
because "a third string would be a typo" is a claim about two files and a comment is not
a check.

- [x] **Step 2: Run it and watch it fail**

`cargo test --manifest-path src-tauri/Cargo.toml --test push` →
`unresolved import stvdy_shell_lib::push`.

**Measured:** `error[E0433]: failed to resolve: could not find `push` in
`stvdy_shell_lib`, 7 errors, `could not compile stvdy-shell (test "push")`. The line
reverted to watch it fail was `pub mod push;` in `src-tauri/src/lib.rs` — with the module
undeclared, the test file cannot see it, which is the state the plan predicts.

- [x] **Step 3: Add the plugin and the seam**

`src-tauri/Cargo.toml`: add `tauri-plugin-notifications = "0.5.0-rc.14"` under
`[target.'cfg(any(target_os = "ios", target_os = "android"))'.dependencies]` (it is
mobile-only; the matrix says Windows is not yet available).

`src-tauri/src/push.rs` holds the pure helpers (`provider_for`, `validate_token`) and a
`register(app)` that asks the plugin for the current token, validates it, and returns it
for the SPA to PUT. `src-tauri/src/lib.rs` registers the plugin under
`#[cfg(any(target_os = "ios", target_os = "android"))]` and adds a `push_token` command.

**One addition to the dependency the plan's version pin alone would have shipped broken:**
the `push-notifications` feature. It is **not** in the crate's defaults, and without it
`register_for_push_notifications` is not compiled at all — it returns
`Error::Io("Push notifications feature is not enabled")`, the Android build ships no
Firebase dependency and the iOS build no APNs registration. A receive path that compiles
and can never receive is exactly the failure this task exists to prevent, so the pin is
`{ version = "0.5.0-rc.14", features = ["push-notifications"] }`.

**`register` is `async`, and that is not cosmetic.** The plugin's mobile half crosses into
the native plugin over `run_mobile_plugin_async`, so `request_permission` and
`register_for_push_notifications` both return futures on iOS/Android (the desktop half of
the same crate is synchronous, which is why the crate's doc page does not show it). The
first draft called them synchronously and failed to compile for `aarch64-apple-ios-sim`
with `expected future, found Result<_, _>` and
`no method named map_err found for opaque type impl Future` — four errors, all in
`src/push.rs`. The command is therefore `async fn push_token` too.

**`register` is `cfg`-gated to iOS/Android and the command is not.** The gate is on the
function that names the plugin, not on the module: `provider_for` and `validate_token`
compile and are tested on macOS, which is the whole reason the dependency is target-gated
rather than global. The seam is in the right place — `cargo check --target
aarch64-apple-ios-sim --lib` exits 0 with the plugin linked, and the pure half is asserted
by four tests on macOS.

- [x] **Step 4: The plugin's config and the credential example**

`tauri.conf.json` gains `plugins.notifications` with the app's identity (no secrets).
`src-tauri/google-services.json.example` documents the shape; the real file is untracked
and supplied at build time (A328). `.gitignore` gains `google-services.json` and
`**/*.p12`/`**/*.mobileprovision`.

**The config block is `{}` and that is correct, not an omission.** `PluginConfig` is
`#[serde(default)]` over a struct whose only field is `#[cfg(target_os = "windows")]`, so
on iOS and Android it has *no* fields: there is no app identity for this plugin to carry
(identity lives in `tauri.conf.json`'s `identifier` and in the store records). The block
is written anyway so the next reader can see the plugin was considered, and so a future
version that gains a field has a place to put it.

- [x] **Step 5: Run it**

`cargo test --manifest-path src-tauri/Cargo.toml --test push` → both tests pass.
`cargo clippy --manifest-path src-tauri/Cargo.toml --all-targets -- -D warnings` clean.

**Measured.** `tests/push.rs`: **4 passed**. The whole shell: **19 unit + 12 cache + 1
config + 4 push = 36 passed, 0 failed**; `cargo clippy --manifest-path
src-tauri/Cargo.toml --all-targets -- -D warnings` exits 0; `rustfmt --edition 2024
--check` is clean on the four files this task touches. **And the plugin is actually
linked, not merely declared**: `tauri ios build --debug --target aarch64-sim` was re-run
with the dependency in place and exits **0**, `** BUILD SUCCEEDED **`, producing
`src-tauri/gen/apple/build/arm64-sim/Stvdy.app` (110 MB, up from 104 MB). That is the
strongest claim available here — the registration itself needs a device and a signing
identity and is **not** verified (A328, A327).

- [x] **Step 6: Commit**

```
git commit -am "Phase 14 Task 2: push receive plugin behind a provider/token seam"
```

**`git commit -am` was NOT used**, for the reason Task 1's Step 6 records: `-a` stages
every tracked modification in the tree, and other slices' work is in it. Explicit paths,
`git diff --cached --stat` checked first.

---

## Task 3: The custom mobile plugin — the background scheduler seam (A315)

**Concern:** one crate that registers `BGTaskScheduler` (iOS) and a `WorkManager`
worker (Android) that call the Phase 8 Rust `sync_once` directly, never the webview.

**Files:**
- New: `src-tauri/plugins/mobile/Cargo.toml`, `build.rs`, `src/lib.rs`, `src/scheduler.rs`
- New: `src-tauri/plugins/mobile/android/**` (Kotlin + manifest + Gradle)
- New: `src-tauri/plugins/mobile/ios/Sources/**` (Swift)
- Modify: `src-tauri/Cargo.toml` (path dependency), `src-tauri/src/lib.rs`,
  `src-tauri/capabilities/mobile.json`
- Test: `src-tauri/plugins/mobile/tests/scheduler.rs`

**Interfaces:**
- Consumes: `stvdy_shell_lib::sync::sync_once` (Phase 8).
- Produces: `MobileExt` (`.init()`), `mobile_scheduler_register(interval_minutes)`.

- [ ] **Step 1: Write the failing test — the seam does not touch the frontend**

`src-tauri/plugins/mobile/tests/scheduler.rs`:

```rust
//! A315's mechanical defence: the background entry points call Rust and never the
//! webview. This is a source assertion because the property is structural — there
//! is no runtime in which a background task could render JS to observe.

#[test]
fn the_scheduler_seam_never_references_the_webview() {
    let source = include_str!("../src/scheduler.rs");
    for forbidden in ["WebviewWindow", "emit(", "eval(", "get_webview"] {
        assert!(
            !source.contains(forbidden),
            "the background seam must not touch the frontend: found {forbidden}"
        );
    }
    assert!(
        source.contains("sync_once"),
        "the seam's whole job is to call the Rust sync engine"
    );
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test --manifest-path src-tauri/plugins/mobile/Cargo.toml` → the crate does not
exist.

- [ ] **Step 3: Create the plugin crate**

`src-tauri/plugins/mobile/Cargo.toml` depends on `tauri` and the shell lib (a path
dependency), `serde`, `tracing`. `src/scheduler.rs` exposes
`pub fn run_background_sync(app: AppHandle) -> Result<(), SchedulerError>` that calls
the shell's `sync_once`. `src/lib.rs` exposes `init()` and a
`mobile_scheduler_register` command that submits a `BGAppRefreshTaskRequest` (iOS) or
enqueues a unique periodic `WorkManager` request (Android).

- [ ] **Step 4: The native halves**

Android (`android/src/main/java/app/stvdy/mobile/`): a `CoroutineWorker` that calls the
plugin's Rust entry via the JNI bridge Tauri generates, a `WorkManager`
`enqueueUniquePeriodicWork` with a 15-minute minimum, and a `FirebaseMessagingService`
whose `onMessageReceived` forwards to the Rust push path. The manifest declares
`POST_NOTIFICATIONS`, `RECEIVE_BOOT_COMPLETED`, `WAKE_LOCK`, `SCHEDULE_EXACT_ALARM`
(A318) and the service.

iOS (`ios/Sources/`): a `MobilePlugin` that registers
`BGTaskSchedulerPermittedIdentifiers` handlers during `setup()` (the only seam that runs
before the launch sequence ends) and resubmits each request from inside its own handler.
`Info.ios.plist` (the merged plist named in Task 1) gains `UIBackgroundModes`
(`fetch`, `processing`, `remote-notification`) and `BGTaskSchedulerPermittedIdentifiers`.

- [ ] **Step 5: Run it**

`cargo test --manifest-path src-tauri/plugins/mobile/Cargo.toml` → pass.
`cargo clippy --manifest-path src-tauri/plugins/mobile/Cargo.toml --all-targets -- -D warnings`.

- [ ] **Step 6: Commit**

```
git commit -am "Phase 14 Task 3: custom mobile plugin — the background sync seam"
```

---

## Task 4: The iOS Share Extension and the App Group (A312)

**Concern:** the app appears in the share sheet for a PDF from Files/Drive, and the file
reaches the app without the extension launching it.

**Files:**
- New: `src-tauri/gen/apple/project.yml.template` (custom XcodeGen template)
- New: `src-tauri/gen/apple/ShareExtension/ShareViewController.swift`
- New: `src-tauri/gen/apple/ShareExtension/Info.plist`
- New: `src-tauri/gen/apple/Info.ios.plist` (the merged plist)
- Modify: `src-tauri/src/lib.rs` (import the App Group container on foreground)
- Test: `src-tauri/tests/share_group.rs`

**Interfaces:**
- Consumes: `bundle.iOS.template` (Task 1), the App Group `group.app.stvdy`.
- Produces: `share::pending_imports() -> Vec<SharedFile>` and
  `share::clear_import(id)`.

- [ ] **Step 1: Write the failing test — the group id is one string**

`src-tauri/tests/share_group.rs`:

```rust
//! The App Group identifier is the contract between the extension and the app. A
//! second spelling on either side is a silent "the file never arrives".

#[test]
fn the_app_group_identifier_is_declared_in_exactly_one_place() {
    let app = include_str!("../src/share.rs");
    let ext = include_str!("../gen/apple/ShareExtension/ShareViewController.swift");
    assert!(app.contains("group.app.stvdy"), "the app side names the group");
    assert!(ext.contains("group.app.stvdy"), "the extension side names the group");
    // Both files are read whole, so the check is that the *same* literal is the
    // only group identifier in the tree, not that a substring exists.
    assert_eq!(app.matches("group.").count(), 1);
    assert_eq!(ext.matches("group.").count(), 1);
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test --manifest-path src-tauri/Cargo.toml --test share_group` →
`cannot find file src/share.rs`.

- [ ] **Step 3: The Share Extension target**

`project.yml.template` is the generated `project.yml` plus a `ShareExtension` target of
`type: app-extension`, `NSExtensionPointIdentifier = com.apple.share-services`, the App
Group entitlement on both targets, and `NSExtensionActivationRule` for
`public.content` / PDF / image. `ShareViewController.swift` copies the shared item into
`FileManager.default.containerURL(forSecurityApplicationGroupIdentifier: "group.app.stvdy")`
and returns (it cannot open the host app). `Info.ios.plist` declares the App Group under
the app's entitlements.

- [ ] **Step 4: The app side**

`src-tauri/src/share.rs` reads the group container's inbox on foreground, returns the
pending files through a `share_pending` command, and deletes them once the SPA confirms
the import. `lib.rs` calls it in `setup` and on resume.

- [ ] **Step 5: Run it**

`cargo test --manifest-path src-tauri/Cargo.toml --test share_group` → pass. Then a
**device** build confirms the app appears in the share sheet (the simulator cannot show
a share sheet for a real file); that is a prerequisite, not a CI step (A327).

- [ ] **Step 6: Commit**

```
git commit -am "Phase 14 Task 4: iOS Share Extension target + App Group import"
```

---

## Task 5: Android share intake and durable file copy (A313, A314)

**Concern:** a file shared *to* the app (or opened via `bundle.fileAssociations`) is
read from its `content://` URI and copied into app storage before the grant expires.

**Files:**
- Modify: `src-tauri/plugins/mobile/android/**` (the `ACTION_SEND`/`ACTION_VIEW` path)
- Modify: `src-tauri/src/share.rs` (the Android arm)
- Test: `src-tauri/tests/share_android.rs`

**Interfaces:**
- Consumes: the fs plugin's URL-based read (`readFile`), `bundle.fileAssociations`.
- Produces: `share::copy_into_storage(uri) -> Result<PathBuf, ShareError>`.

- [ ] **Step 1: Write the failing test — a concrete MIME type, never `*/*`**

```rust
//! The matrix verified `mimeType: "*/*"` never matches a concrete intent, so the
//! config must carry concrete types. This reads the config as data.

#[test]
fn every_file_association_declares_a_concrete_mime_type() {
    let config: serde_json::Value =
        serde_json::from_str(include_str!("../../tauri.conf.json")).unwrap();
    let assoc = config["bundle"]["fileAssociations"].as_array().unwrap();
    assert!(!assoc.is_empty());
    for entry in assoc {
        let mime = entry["mimeType"].as_str().unwrap();
        assert_ne!(mime, "*/*");
        assert!(mime.contains('/'), "{mime} is not a concrete type");
    }
}
```

- [ ] **Step 2: Run it and watch it fail**

`cargo test --manifest-path src-tauri/Cargo.toml --test share_android` → if Task 1 left
`fileAssociations` empty, the `assert!(!assoc.is_empty())` fails.

- [ ] **Step 3: Implement the intake**

The Kotlin `onNewIntent`/`onCreate` path reads the `content://` URI from `ACTION_SEND` /
`ACTION_VIEW`, and `share::copy_into_storage` reads it with the **URL-based** fs API
(never `stat`/`exists`, which fail on `content://`) and writes a copy under the app's
data dir. The copied path is what the upload queue holds; the original grant is allowed
to die.

- [ ] **Step 4: Run it**

`cargo test --manifest-path src-tauri/Cargo.toml --test share_android` → pass. A device
build confirms a real share delivers a readable URI (prerequisite).

- [ ] **Step 5: Commit**

```
git commit -am "Phase 14 Task 5: Android share intake with an immediate durable copy"
```

---

## Task 6: Push token registration and deep-link round trip in the SPA (A320, A329)

**Concern:** after login the app registers its push token with the device row, and a
tapped notification or a universal link lands on the right screen — including a cold
start.

**Files:**
- Modify: `app/src/platform/types.ts` (a `PushApi`), `app/src/platform/mobile.ts`
- New: `app/src/features/notifications/registerPush.ts`,
  `app/src/features/notifications/NotificationRouter.tsx`
- Modify: `app/src/router.tsx` (the notification route handler)
- Test: `app/src/features/notifications/registerPush.test.tsx`,
  `app/src/platform/adapter.test.ts`

**Interfaces:**
- Consumes: `api.put('/devices/:id/push-token', { provider, token })`; the shell's
  `push_token` command; `platform.deepLinks`.
- Produces: `registerPush(deviceId)`, `onNotificationTap(intent)`.

- [ ] **Step 1: Write the failing test**

```tsx
// app/src/features/notifications/registerPush.test.tsx
it('registers the token against the current device and re-registers on refresh', async () => {
  const { calls } = installFakePlatform()
  mockFetch({ 'PUT /devices/d1/push-token': { status: 204 } })
  await registerPush('d1')
  expect(calls).toContainEqual({
    method: 'PUT',
    path: '/devices/d1/push-token',
    body: { provider: 'apns', token: 'tok-1' },
  })
})
```

and an adapter case that `platform.push.onToken` forwards the shell's token event.

- [ ] **Step 2: Run it and watch it fail**

`npm --prefix app test -- registerPush` → `Cannot find module`.

- [ ] **Step 3: Implement**

`platform.push` wraps the shell's token event and `onNotification`. `registerPush` calls
it once after login and on every token-refresh event, PUTting `provider`/`token` (A329).
`NotificationRouter` maps a notification's payload to a route (`workspace`,
`task`, `reminder`) and navigates, buffering the tap when the app is cold-started
(A316's "tap-to-open is the only guaranteed surface").

- [ ] **Step 4: Deep links**

Add `applinks:app.stvdy` to `bundle.iOS.associatedDomains` and rely on tauri-build's
`autoVerify` Android filters (A320). A test asserts a `stvdy://` URL and an
`https://app.stvdy/...` universal link both classify to the same `DeepLinkIntent`.

- [ ] **Step 5: Run it**

`npm --prefix app test -- notifications` → pass; `npm --prefix app run build` clean.

- [ ] **Step 6: Commit**

```
git commit -am "Phase 14 Task 6: push token registration + deep-link/notification routing"
```

---

## Task 7: Secure storage and local notifications, verified on a device (A319, A318)

**Concern:** the refresh token survives a restart in the keychain/keystore, and a
scheduled local reminder fires while the app is closed — with the platform's inaccuracy
documented.

**Files:**
- Modify: `docs/ARCHITECTURE.md` § Native prototypes (Phase 14 device results)
- Modify: `docs/RELEASE.md` (the reminder-accuracy note)
- Test: `app/src/features/notifications/localReminder.test.ts` (the SPA scheduling path)

**Interfaces:**
- Consumes: `platform.secureStore` (Phase 1), `platform.notifications.schedule`.
- Produces: no new code beyond a device verification and one SPA test.

- [ ] **Step 1: Write the failing test — scheduling is best-effort, and the UI says so**

```ts
it('schedules a local reminder and never claims exact delivery', async () => {
  const { calls } = installFakePlatform()
  await scheduleReminder('Drink water', new Date('2026-10-12T09:00:00Z'))
  expect(calls.some((c) => c.path === 'notifications.schedule')).toBe(true)
  // The copy under the reminder must not promise minute accuracy (A318).
  expect(screen.queryByText(/exactly|on the minute/i)).toBeNull()
})
```

- [ ] **Step 2: Run it and watch it fail**

`npm --prefix app test -- localReminder` → the module does not exist.

- [ ] **Step 3: Implement and verify**

Wire `platform.notifications.schedule` into the reminder UI; on a **device**, store a
token, kill the app, and confirm the keychain read on relaunch and a scheduled
notification firing while closed. Record pass/fail in `docs/ARCHITECTURE.md`. Add the
Android Auto Backup caveat (a restored install loses Keystore keys → re-login) to
`docs/RELEASE.md`.

- [ ] **Step 4: Commit**

```
git commit -am "Phase 14 Task 7: secure storage + local notifications verified on device"
```

---

## Task 8: `scripts/check-no-purchase-ui.mjs` — prove the absence in the artifact (A321)

**Concern:** the shipped mobile bundle contains no checkout, no price, no off-store
purchase link — proved by reading the bytes, not the component.

**Files:**
- New: `scripts/check-no-purchase-ui.mjs`
- Modify: `app/package.json` (a `check:no-purchase-ui` script), `.github/workflows/ci.yml`
- Test: `scripts/check-no-purchase-ui.test.mjs` (a fixture that must fail)

**Interfaces:**
- Consumes: the **store build** under `app/dist` (`TAURI_ENV_PLATFORM=ios npm run build`,
  or `npm run build:mobile` for `dist-mobile`), `config/billing_policy.toml`.
- Produces: exit 0 with `no purchase UI in N shipped file(s)` and the artifact's
  `stvdy-build-target` stamp, or exit 1 with a list.

- [x] **Step 1: Write the failing test — a planted marker must be caught**

```js
// scripts/check-no-purchase-ui.test.mjs
import { test } from 'node:test'
import assert from 'node:assert/strict'
import { findOffenders } from './check-no-purchase-ui.mjs'

test('a bundle that carries a price is rejected', () => {
  const files = { 'assets/app.js': 'const price = "Rp 10.000"' }
  const offenders = findOffenders(files)
  assert.ok(offenders.length > 0, 'Rp must be caught')
})

test('a clean bundle passes', () => {
  assert.deepEqual(findOffenders({ 'assets/app.js': 'export const x = 1' }), [])
})
```

- [x] **Step 2: Run it and watch it fail**

`node --test scripts/check-no-purchase-ui.test.mjs` → `Cannot find module`.

- [x] **Step 3: Implement, in the `check-no-cdn.mjs` shape**

Export `findOffenders(files: Record<string,string>)` and, when run as a script, read the
store build under `app/dist`, build the map, and fail on the forms that *are* a purchase
path: `Rp`/`IDR` followed by digits, `billing/checkout`, and `buy(`/`checkout(`/`purchase(`.
It also reads `config/billing_policy.toml` and fails if `allow_purchase_ui`, `show_prices` or
`allow_purchase_cta_in_locked_messages` is `true` for `ios` or `android`, or if either
block or key is missing.

**The marker set this plan first named was too wide, and the shipped one is narrower on
purpose.** `QRIS` and `Upgrade to` are not purchase paths: `Upgrade to {{plan}}` is a locale
string the mobile policy never renders, and the styleguide's own description of the two
`LockedFeature` variants names checkout. Flagging them would fail the *correct* build until
someone added an allowlist, which is how a gate becomes noise and then gets deleted — the
reasoning `check-no-cdn.mjs` states for looking at loads rather than URLs. The narrower set
is pinned in both directions by `scripts/check-no-purchase-ui.test.mjs`, which asserts the
legitimately-shipped strings are **not** offenders as well as that planted ones are.

- [x] **Step 4: Run it**

```
TAURI_ENV_PLATFORM=ios npm --prefix app run build   # the store build, into app/dist
node scripts/check-no-purchase-ui.mjs               # exit 0, prints the count + the stamp
node --test scripts/check-no-purchase-ui.test.mjs
```

A plain `npm --prefix app run build` writes the **web/desktop** build into the same
directory, and the check refuses it (`stvdy-build-target="full"`) — which is the honest
result and the reason the store build is keyed on the shell's own platform variable rather
than on a convention. See `docs/ARCHITECTURE.md` § Store-absence checks must validate the
artifact that actually ships.

- [x] **Step 5: Wire it into CI** after the web build job (A321).

- [x] **Step 6: Commit**

```
git commit -am "Phase 14 Task 8: bundle inspection proves no purchase UI in the mobile build"
```

---

## Task 9: The read-only plan screen and the mobile `/billing` gate (A322)

**Concern:** on iOS/Android `/billing` shows the plan and the usage the app computes,
with no purchase path anywhere in the mobile router.

**Files:**
- New: `app/src/features/billing/MobilePlanScreen.tsx`
- Modify: `app/src/router.tsx`
- Modify: `app/src/i18n/locales/{en,id}.json` (the plan-screen keys)
- Test: `app/src/features/billing/MobilePlanScreen.test.tsx`,
  `app/src/router.test.tsx`

**Interfaces:**
- Consumes: `platform.kind()`, `policyFor`, the Phase 13 plan/usage query.
- Produces: `<MobilePlanScreen/>` and a router element chosen by platform.

- [x] **Step 1: Write the failing test**

```tsx
it('renders the plan and the computed usage, and no purchase control', async () => {
  installFakePlatform({ kind: 'ios' })
  renderWithProviders(<MobilePlanScreen />)
  expect(await screen.findByText(/Pro/)).toBeInTheDocument()
  expect(screen.queryByRole('link', { name: /upgrade/i })).toBeNull()
  expect(screen.queryByText(/Rp/)).toBeNull()
})

it('routes /billing to the read-only screen on android', () => {
  installFakePlatform({ kind: 'android' })
  expect(routeElementFor('/billing').type).toBe(MobilePlanScreen)
})
```

- [x] **Step 2: Run it and watch it fail**

`npm --prefix app test -- MobilePlanScreen` → `Cannot find module`.

- [x] **Step 3: Implement**

`MobilePlanScreen` shows the plan name, credits remaining and the usage numbers the app
computes (from Phase 13's query), and the neutral `lock.notOnPlan` copy. `router.tsx`
picks the element: `platform.kind()` in `('ios','android')` → `MobilePlanScreen`, else
`PlanUsageScreen`.

**That choice is a run-time one, and it does not remove any bytes.** It is what makes the
screen testable and correct on a device, and it is *not* what keeps the checkout out of the
store build: `platform.kind()` is read at run time, so both route elements stay in one
chunk. The byte-level exclusion is Phase 14's `__PURCHASE_UI__` `define` (A322), added when
`scripts/check-no-purchase-ui.mjs` was found red on exactly this. The original text of this
step claimed the component "is therefore not imported on the mobile path, so it is
tree-shaken out", which was false until that `define` existed.

- [x] **Step 4: Run it**

`npm --prefix app test -- billing` → pass; then `node scripts/check-no-purchase-ui.mjs`
still exits 0 (the new screen adds no marker).

- [x] **Step 5: Commit**

```
git commit -am "Phase 14 Task 9: read-only mobile plan screen + the /billing platform gate"
```

---

## Task 10: The adaptive phone/tablet UI and the phone-viewport measurements (A323, A324)

**Concern:** the phone and tablet shells render correctly, and gate 5's checks are
measured on a phone viewport — including the ≥44px target check, at its most literal.

**Files:**
- New: `scripts/measure-mobile.mjs`, `app/src/components/layout/Shell.test.tsx` (a
  phone/tablet render case)
- Modify: `app/package.json` (`puppeteer-core` devDependency, a `measure:mobile` script)
- Test: `app/src/lib/useFormFactor.test.ts` (the 768/1024 boundaries)

**Interfaces:**
- Consumes: the running dev server (`npm --prefix app run dev`), `CHROME_PATH`.
- Produces: a pass/fail report per check.

- [x] **Step 1: Write the failing test — the boundaries are exact**

```ts
it('switches shell at 768 and 1024, matching the CSS md/lg', () => {
  expect(formFactorForWidth(767)).toBe('phone')
  expect(formFactorForWidth(768)).toBe('tablet')
  expect(formFactorForWidth(1023)).toBe('tablet')
  expect(formFactorForWidth(1024)).toBe('desktop')
})
```

- [x] **Step 2: Run it and watch it fail**

`npm --prefix app test -- useFormFactor` → the hook is not exported in a testable form
(it is; this passes — if so, the *new* case is the Shell render at 390px, which fails
until the phone shell is asserted).

- [x] **Step 3: Implement `measure-mobile.mjs`**

`puppeteer-core` with `executablePath` from `CHROME_PATH` or the same probe list as
`crates/server/src/render/chrome.rs` (A323). It launches Chrome at 390×844 (device scale
3), navigates to `http://localhost:5173`, and:

1. asserts every element matching `a,button,input,select,textarea,[role=button],[role=tab]`
   has a `getBoundingClientRect()` of ≥44×44 CSS px;
2. reads computed `color`/`background-color` for the default dark and light themes and
   asserts WCAG AA contrast (4.5:1 body, 3:1 large);
3. sets `document.documentElement.style.fontSize = '32px'` (200%) and asserts no element
   scrolls its container or overlaps its neighbour;
4. prints one line per check and exits non-zero on any failure.

- [x] **Step 4: Run it**

```
npm --prefix app run dev &            # or `npm --prefix app run preview`
CHROME_PATH=/Applications/Google\ Chrome.app/Contents/MacOS/Google\ Chrome \
  node scripts/measure-mobile.mjs
```

**Prerequisite: a Chrome binary.** With none present the script fails loudly, like the
PDF acceptance test, rather than skipping.

- [x] **Step 5: Commit**

```
git commit -am "Phase 14 Task 10: adaptive shell + phone-viewport measurement (gate 5)"
```

---

## Task 11: TestFlight + Play internal testing, the mobile checklist, and the gates

**Concern:** the store channels are set up, the checklist exists with pass/fail columns,
and gates 5/6/7 are run on a phone viewport with quoted output.

**Files:**
- Modify: `docs/RELEASE.md` (the mobile checklist + the store-policy mobile specifics)
- Modify: `docs/ARCHITECTURE.md` (§ Native shims chosen)
- Modify: `docs/ASSUMPTIONS.md` (A310–A329)
- New: `scripts/smoke-phase14.sh`
- Test: `app/src/features/a11y.test.tsx` (an Indonesian phone-shell case)

**Interfaces:**
- Consumes: everything above.
- Produces: the exit checklist evidence and the docs.

- [ ] **Step 1: Write the failing test — Indonesian on the phone shell**

```tsx
it('renders the Indonesian workspace list on the phone shell with no clipped control', async () => {
  i18n.changeLanguage('id')
  installFakePlatform({ kind: 'ios', width: 390 })
  renderWithProviders(<WorkspaceListScreen />, { route: '/workspaces' })
  expect(await screen.findByText('Ruang kerja')).toBeInTheDocument()
  await expectNoA11yViolations(document.body)
})
```

- [ ] **Step 2: Run it and watch it fail**

`npm --prefix app test -- a11y` → the Indonesian phone-shell case fails until the
locale key and the phone shell are both present.

- [ ] **Step 3: The checklist and the smoke script**

`docs/RELEASE.md` gains a **Mobile test checklist** table (each row: device build both
platforms; push token registers; tap-to-open cold + warm; deep link round trip; share
import iOS + Android; background sync fires or the foreground fallback is honest; the
bundle inspection passes; the three gates on a phone viewport) with pass/fail columns
filled at submission, plus the store-policy specifics (the absence proof, the
TestFlight/Play channel, the update path). `scripts/smoke-phase14.sh` runs the checks a
machine can run.

- [ ] **Step 4: Run the gates**

- Gate 5: `node scripts/measure-mobile.mjs` output quoted (44px, contrast both themes,
  200% scale).
- Gate 6: the Indonesian phone-shell test output quoted.
- Gate 7: the mobile plan screen shows the computed usage and no price; the composer
  hint is present.

- [ ] **Step 5: Docs**

`docs/ARCHITECTURE.md` § Native shims chosen records: push plugin + fallback (A310),
the one custom plugin (A311), the Share Extension + App Group (A312), the scheduler
seam (A315), the bundle inspection (A321). `docs/ASSUMPTIONS.md` gains A310–A329.

- [ ] **Step 6: Commit**

```
git commit -am "Phase 14 Task 11: store channels, mobile checklist, gates, docs"
```

---

## Phase 14 exit checklist

| # | Check | Command / evidence | Pass condition |
|---|---|---|---|
| 1 | Workspace quality gate | `cargo fmt --check` · `cargo clippy --workspace --all-targets -- -D warnings` · `cargo test --workspace` | clean |
| 2 | Shell + plugin | `cargo test --manifest-path src-tauri/Cargo.toml` · `…/plugins/mobile/Cargo.toml` | pass |
| 3 | Web | `npm --prefix app test` · `npm --prefix app run build` | pass |
| 4 | iOS build | `tauri ios build --debug --target aarch64-sim` | exit 0, bundle produced |
| 5 | Android build | `tauri android build --debug` | exit 0, APK produced |
| 6 | **No purchase UI** | `node scripts/check-no-purchase-ui.mjs` | exit 0, prints the file count |
| 7 | Push token registers | device: log + `push_tokens` row | token present, provider correct |
| 8 | Deep links round-trip | cold start + warm, `stvdy://` and universal link | both classify to the same intent |
| 9 | Gate 5 | `node scripts/measure-mobile.mjs` | 44px, contrast both themes, 200% scale all pass. **13 surfaces** — the list, and the surfaces it does *not* reach with the reason for each, is in A323 above; the two it cannot reach from the tab bar (`/planner`, `/syllabus`) are Phase 11's own gate-5 leg |
| 10 | Gate 6 | Indonesian phone-shell test | pass, output quoted |
| 11 | Gate 7 | mobile plan screen + composer hint | no price, computed usage, hint present |
| 12 | Docs | RELEASE/ARCHITECTURE/ASSUMPTIONS updated | present |

---

## Self-Review

- **Spec coverage:** every item the roadmap's Phase 14 line names is a task: Tauri
  mobile builds (Task 1), phone/tablet adaptive UI (Task 10), read-only plan screen with
  no purchase UI (Tasks 8–9), push plugins with token registration (Tasks 2, 6),
  universal links/app links (Task 6), secure storage (Task 7), file picker + share-sheet
  import (Tasks 4–5), local notifications (Task 7), TestFlight + Play internal testing
  (Task 11), mobile test checklist + store-policy risk note in `docs/RELEASE.md`
  (Task 11). The risk-register row ("treat push + share extension + background scheduler
  as **one native-work package**") is honoured: A311 makes the scheduler and Android
  intake one plugin, A310 isolates push, and A312 is the share extension — one package,
  three mechanisms, with the split justified by the platform mechanism rather than by
  convenience.
- **Placeholders:** none. The push plugin version is pinned (A310); the fallback is
  named; every device-only step names its prerequisite and what degrades to a simulator
  (A317, A327); `google-services.json` is a documented example, not a committed secret
  (A328).
- **A-numbering:** the roadmap reserves A250–A269 for Phase 11 and A310+ is the next
  free range, so this plan uses **A310–A329** (the task's "starting at A310"). This is
  the one place the task's literal number and a naive reading of the roadmap's blocks
  could disagree; the number used is the next free one after Phase 11's A269 and Phase
  12/13's reserved A270–A309.
- **Type consistency:** `Platform.push` (Task 6), `provider_for`/`validate_token`
  (Task 2) and `push_tokens.provider` (`devices.rs`) all use the same two strings
  (`apns`, `fcm`); the App Group string (`group.app.stvdy`) is asserted unique in Task 4;
  the phone breakpoints are the single `useFormFactor` pair (768/1024) asserted in
  Task 10.
- **The one thing this plan cannot make true:** a physical device and a store account.
  APNs/FCM registration, the share-sheet row and the store submission are prerequisites
  named in A327 and the checklist, not claims.

## Execution notes

- Tasks 1–3 are the build + native skeleton; nothing in Tasks 4–11 compiles against a
  device, so the order holds on a laptop except where a task says otherwise.
- Task 1's Android half is blocked without an Android SDK; the plan records that rather
  than working around it, and the iOS half still runs.
- Run `just check-shell` (the `src-tauri` trio) after every shell task; the shell is
  excluded from the workspace, so `cargo test --workspace` never sees it.
