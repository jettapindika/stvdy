# Release

**This is the normative release document** (A354). Every operational procedure lives either here
or in `docs/OPERATIONS.md`, and nowhere else:

- **This file** is what is distributed, under which policy, with which pinned components, and
  what must be true before a release is published — the release process, the store checklists and
  the launch checklist.
- **`docs/OPERATIONS.md`** is the day-two runbook: the production host checklist, the deploy
  sequence, backups and the restore drill, monitoring, the breach runbook and retention. It is
  the shorter document on purpose, because it is the one an operator opens during an incident.

Filled by Phase 15, except the policy notes below which constrain earlier phases.

## Desktop distribution

Direct download: signed and notarized macOS universal build, signed Windows
installer. No Mac App Store, no Microsoft Store.

**The macOS job, and the four commands that make "signed and notarized" a transcript rather
than an adjective.** Runner: `macos-15`. The job runs
`tauri build --target universal-apple-darwin` — with `bundle.macOS.hardenedRuntime` and
`bundle.macOS.entitlements` read from the committed `tauri.conf.json` rather than passed on
the command line, because two places to change is how one of them ends up stale — then runs
`scripts/notarize-macos.sh` on the `.dmg` with the notary credentials, then asserts all four
of these **before it uploads anything**. A release that publishes an unverified artifact is
worse than one that fails:

```bash
codesign --verify --deep --strict --verbose=2 "$APP"            # the signature verifies
spctl -a -vvv -t install "$APP"                                 # Gatekeeper accepts it
xcrun stapler validate "$DMG"                                   # the ticket is stapled
lipo -archs "$APP/Contents/MacOS/$(/usr/libexec/PlistBuddy -c 'Print :CFBundleExecutable' "$APP/Contents/Info.plist")"
```

Each one fails for a different reason and none of the four implies another: a valid
signature with no stapled ticket launches only with a network, a stapled `.dmg` whose
`.app` is unstapled fails after the user copies the app out, and a single-architecture
binary passes the first three and fails only on the machine it was not built for.
`scripts/notarize-macos.sh` is where the double staple happens, and its own two traps are
documented there: `notarytool submit --wait` exits 0 for a **rejected** submission (so the
script parses `status: Accepted` and prints the log id otherwise), and the ticket must be
stapled to the `.app` **before** the dmg is rebuilt.

- macOS: `universal-apple-darwin` target, `codesign` + `notarytool`, hardened
  runtime, minimum macOS 11.0, **and the notarization ticket stapled into both the `.app` and
  the `.dmg`** (A333) — an unstapled dmg fails on a machine with no network.
- Windows: `nsis` installer, `perMachine` install mode, code-signed, **SHA-256, with an RFC 3161
  timestamp** (A334). Without a certificate the installer builds unsigned and SmartScreen warns
  on every install; that is a recorded degraded state, not a defect.
- Updater: `tauri-plugin-updater` 2.13.3, **desktop only** — the plugin's own platform matrix is
  `ios: none, android: none`, so a mobile build that enabled the capability would fail to compile
  rather than expose an API that does nothing, and mobile updates ship through TestFlight and
  Play. The manifest endpoint and the minisign public key live in `tauri.conf.json`'s
  `plugins.updater`; the **private** key is a GitHub Actions secret used by `release.yml` only, and
  `scripts/verify-release-artifacts.sh` refuses to run if it is present in its environment (A332).
  Until the operator pastes their real public key, the committed placeholder means every signature
  fails to verify — a safe-but-useless updater, not a startup panic. The update is offered, never
  applied silently, and the panel renders only what the manifest carried (A336).

## Mobile distribution

TestFlight / App Store for iOS, Play internal testing / Google Play for Android.
A direct-APK download from the website is an allowed alternative Android channel.
Mobile updates ship through the stores; the Tauri updater does not support them.

**The bundle's own web assets must carry no purchase affordance**, and that is a
command rather than a promise: `scripts/check-mobile-no-purchase.sh` runs the
absence check against the assets a build packages, and
`scripts/check-mobile-no-purchase.sh path/to/artifact.apk` runs it against an
extracted store artifact. Both store checklists below cite it; what it does and
does not inspect is in § Store submission checklists.

### The absence proof, and the two counts it produces

Two commands, and both are needed — the second reads what the first produced:

```
TAURI_ENV_PLATFORM=ios npm --prefix app run build   # the store bundle, into app/dist
node scripts/check-no-purchase-ui.mjs               # exit 0, and prints the file count
```

`TAURI_ENV_PLATFORM=ios` is what makes it a **store** build (A322): `vite.config.ts`
stamps `index.html` with `<meta name="stvdy-build-target" content="store">`, defines
`__PURCHASE_UI__` as `false`, and the checkout route and the module behind it are dropped at
build time rather than hidden at run time. Without it the same command produces the
web/desktop bundle, which the check **refuses** — a `full` stamp fails even when its bytes
are clean, and a bundle with no stamp at all fails too, because an artifact the check cannot
identify is one whose cleanliness proves nothing.

**The two counts, measured on this tree:** the store build contains **0** occurrences of
`billing/checkout`; the web/desktop build contains **5**. That difference is the claim in
its whole form — the strings live in the one entry chunk every platform would otherwise
install, and a build-time `define` is what removes them, where a `platform.kind()` check
could only hide them. The check prints `no purchase UI in 3 shipped file(s)` and exits 0.
`scripts/check-mobile-no-purchase.sh` is the same check pointed at a packaged archive
instead of `app/dist`; on this machine it has no `.ipa` or `.apk` to read, which is row
M9's caveat.

### Building the APK: two things the generated project does not do for you

Both were found by building one, not by reading about it, and both apply to the signed
(`release.yml`) and unsigned (`.github/workflows/unsigned-artifacts.yml`) paths alike.

- **`src-tauri/tauri.js` has to exist before Gradle runs.** The generated
  `buildSrc/src/main/java/<id>/kotlin/BuildTask.kt` invokes
  `node tauri android android-studio-script --release --target <abi>` with its working
  directory set to `src-tauri/`. Node resolves that first argument as a **path**, not as a
  module — `node tauri` needs a file called `tauri(.js)` in the working directory, and
  `node_modules` is never consulted — so in this repository, whose npm project is `app/`,
  every `rustBuild*Release` task fails with `Cannot find module '<repo>/src-tauri/tauri'`
  after ~20 minutes of Gradle. Both workflows write a one-line forwarder to the real CLI.
  Measured: `node tauri --version` fails in an empty directory and prints
  `tauri-cli 2.12.2` with the file present.
- **The release build type has no signing configuration.** The generated
  `app/build.gradle.kts` has no `signingConfigs` block and its `release` build type sets no
  `signingConfig`, so a release APK comes out unsigned and Gradle refuses the build.
  `keystore.properties` alone does nothing — in `@tauri-apps/cli` 2.12.2's own template the
  file's only mention is a `.gitignore` entry. The unsigned workflow appends the Gradle block
  (idempotently, so a committed project carrying its own is left alone); a committed project
  must carry one, or `release.yml` builds an unsigned release APK and fails.

**The API origin is a build input, not a setting.** `VITE_API_BASE_URL` is inlined by Vite
and falls back to `http://localhost:8080`, so an artifact built without it installs and then
fails every request. `.github/workflows/unsigned-artifacts.yml` takes it as a required input
and refuses the fallback; the same value must be in the server's CORS policy only if a
**browser** serves the SPA — the three Tauri webview origins are always allowed
(`crates/server/src/routes/mod.rs`).

**A Tauri APK carries its frontend inside the native library, not in `assets/`.**
`tauri::generate_context!()` embeds the bundle through `rust-embed`, so
`check-mobile-no-purchase.sh path/to.apk` has to recover it from
`lib/<abi>/libstvdy_shell_lib.so` before it can scan anything — scanning the archive's members
finds only `assets/tauri.conf.json` and the dexopt profile. `scripts/extract-embedded-assets.mjs`
does the recovery (`[name][brotli stream]` per file, `rust-embed` 8.13.0's layout, verified on a
real 11 MB library) and **fails closed**: a library whose bundle cannot be read is a failure,
not a skip. Until it existed the APK mode failed on **every** real artifact, which is a gate
that cannot pass on the thing it gates — found by building one whose bundle was stamped `full`
and carried six `billing/checkout` strings, exactly what the gate is for.

### The channel, and the update path (A325)

- **iOS — TestFlight, then App Store review.** The channel is App Store Connect; the
  account is row A6 and `HANDOVER.md` § Credentials. `docs/ARCHITECTURE.md` § Native shims
  chosen records what the build adds for it (the APNs entitlement, the share extension).
- **Android — Play internal testing, then production.** The channel is the Play Console;
  the account is row G7 and `HANDOVER.md` § Credentials. A direct-APK download from the
  website is the allowed alternative channel, and it is signed by a **different key** than
  the Play build (A352, row G6).
- **Updates ship through the stores.** `tauri-plugin-updater` declares
  `ios: none, android: none`, and `src-tauri/Cargo.toml` keeps it inside a
  `cfg(any(target_os = "macos", …))` target block, so a mobile build cannot link it: it
  would fail to compile rather than expose a button that does nothing.
  `mobilePlatform.checkForUpdate()` returns `unsupported` and `installUpdate()` throws, so
  the Settings → Updates panel is honest on a phone. The **desktop** updater is Phase 15's
  (A332) and is described in § Desktop distribution.

### Mobile test checklist

**Every row is `code`, `user` or `blocked`, and the Result column is filled from what was
actually observed** (A326). A checklist with an empty result column is a plan, and the
difference matters on the day of submission: the rows that cannot be filled without a
device and an account are the ones a phase hands to the user rather than assumes.

**No device, no signing identity, no Android SDK and no store account exist on the machine
this was filled on.** A row whose evidence needs one is `user` or `blocked` and says so;
none of them is a pass, and none of them is left blank.

| # | Item | Kind | Evidence / owner | Result |
|---|---|---|---|---|
| M1 | **iOS build** — the shell builds and produces a bundle | **code** | `tauri ios build --debug --target aarch64-sim` → `src-tauri/gen/apple/build/arm64-sim/Stvdy.app` | **PASS with one caveat, stated:** the bundle is real (`Stvdy.app`, built 2026-10-12, a Mach-O arm64 executable, linked against the real `iphonesimulator27.0` SDK), and **the CLI's own runtime precondition was bypassed to get there** — `ensure_ios_runtime_installed()` gates on `xcrun simctl list runtimes` finding an iOS runtime, and none is installed, so the run used a three-line `xcrun` shim answering that one query with a synthetic runtime (`docs/superpowers/plans/2026-10-11-phase-14-mobile.md:665`; nothing in the repository was changed by it). The honest fix is `xcodebuild -downloadPlatform iOS`, a multi-GB download, and CI's `macos-15` leg must run it before this command. It is a **simulator** bundle, not a device one: a signed device `.ipa` needs the identity in A6, so that half is `user` |
| M2 | **Android build** — `tauri android build --debug` produces an APK | **blocked** | No Android SDK on this machine (`~/Library/Android/sdk` is absent) and no `src-tauri/gen/android/` | **NOT RUN.** The toolchain is absent, so no `.apk` exists to inspect or install. `gen/android/` is generated by `tauri android init`, which needs the SDK |
| M3 | **Push token registers** — the OS mints a token and `push_tokens` gains a row with the right provider | **user** | Needs an APNs `.p8` key, a Firebase project and a device (A328). The seam is `src-tauri/src/push.rs` (`provider_for`/`validate_token`/`register`) plus `PUT /devices/{id}/push-token` | **NOT VERIFIABLE HERE.** No key, no Firebase project, no device. The server route exists (A329) and the shell module is present; neither is a token arriving |
| M4 | **Tap-to-open, cold and warm** — a push tap opens the right surface, including from a cold start | **user** | Needs a device and a delivered push; the JS surface is `platform.onNotification` / the deep-link handler | **NOT VERIFIABLE HERE.** A cold-start buffer cannot be exercised without an OS that delivers the notification |
| M5 | **Deep-link round trip** — `stvdy://` and the universal link classify to the same intent, cold and warm | **user** | The classification is `code` (`src-tauri/src/deeplink.rs`'s `classify`, and the SPA adapter's tests); the OS hand-off needs a device and the `associatedDomains`/`autoVerify` setup (A320) | **HALF CLOSED, HALF NOT.** The classifier is tested and its intent union is pinned; the round trip — the OS delivering the URL to a running or cold app — needs a device |
| M6 | **Share import, iOS** — the Share Extension writes into the App Group and the app imports it on next foreground | **blocked** | **Not built.** The extension target is absent: no app-extension entry and no `group.app.stvdy` entitlement in `gen/apple/project.yml.template`. A312/A313 record the design (the hand-wired `bundle.iOS.template` target over the generated XcodeGen project) | **NOT BUILT.** The extension cannot launch its host app, so it needs the App Group and a real device to observe at all |
| M7 | **Share import, Android** — the `ACTION_SEND` intake copies the `content://` URI before the grant dies | **user** | The intake is **committed** (`83dc02d`): the Kotlin `ShareIntake.kt` writes the inbox itself and `MobilePlugin.kt` carries the two lifecycle hooks, the intent filters are hand-written in the plugin's `AndroidManifest.xml`, and the crate is registered at `src-tauri/src/lib.rs:141`. **Three claims this row used to make are false at HEAD and are corrected rather than left to mislead:** the plugin's `src/intake.rs` no longer exists (the Kotlin half writes the inbox, because on a cold start there is no caller for a Rust-side inbox writer), the crate is tracked, and it *is* registered. | **NOT VERIFIABLE HERE.** A `content://` grant is transient and the intake needs a device to exercise; the Kotlin is also unbuilt, because `~/Library/Android/sdk` does not exist and `ANDROID_HOME` is unset, so there is no `gradle` or `kotlinc`. What `tests/share_android.rs` asserts (**4 passed**) is the contract between the files — the three MIME types in both `tauri.conf.json` and the manifest, the inbox name, and the sidecar keys the two languages must agree on |
| M8 | **Background sync fires, or the foreground fallback is honest** | **user** | The fallback is `code` (the Phase 8 sync-status surface and its "Last synced …" line); a background window firing needs a device (A315, A317) | **THE FALLBACK HALF IS CLOSED; THE FIRING HALF IS NOT.** A background window cannot be observed here, and `BGTaskScheduler`/`WorkManager` scheduling is discretionary (A317), so the row is typed `user` — the half a device would add is the firing |
| M9 | **The bundle inspection passes** | **code** | `TAURI_ENV_PLATFORM=ios npm --prefix app run build` then `node scripts/check-no-purchase-ui.mjs` | **PASS.** Exit 0, `no purchase UI in 3 shipped file(s)`, stamp `store`, **0** occurrences of `billing/checkout` against the web build's **5** |
| M10 | **The three gates on a phone viewport** | **code** | Gate 5 `node scripts/measure-mobile.mjs`; gate 6 the Indonesian phone-shell case in `app/src/features/a11y.test.tsx`; gate 7 `app/src/features/billing/MobilePlanScreen.test.tsx`'s no-price case | **PASS on all five of gate 5's criteria, with nothing left over.** Gate 5 measured at 390×844, both themes, 100%/200%: **0** undersized, **0** text pairs below requirement, **0** overlaps, **0** clipped, **0** page-width overflows, lowest contrast **4.97:1**. **Re-measured 2026-10-12 at the full walk's current size — 24 surfaces, 96 combinations, 1232 targets, 3380 pairs** (the earlier 80/1056/3048 was the same walk before the settings slice's edit landed and before Phase 4's four study surfaces were added; it is the same command and the same pass condition, so the numbers move with the surface list rather than with a different check). The keyboard leg is green on every surface too: **1188 Tab stops, 0 unreached, 0 without a focus ring, 0 trapping, 144 excused by design**. **The 22 clipped elements this row used to name are gone:** the re-measurement reports **0 clipped and 0 overlapped on every surface at both scales**, exit 0, so the walk now has no failure to report rather than a failure that is somebody else's — the clipped count on the checkout screens was fixed after Phase 13's gate recorded it as reported-not-fixed. Gate 6: the Indonesian case passes (`npm --prefix app test -- a11y` → 12 passed, 0 failed). Gate 7: the phone plan screen renders the computed usage and no price |
| M11 | **TestFlight / Play upload** — the build reaches a real testing track | **user** | Needs the accounts in A6/G7 and a signed artifact from M1/M2 | **NOT DONE.** No store account exists here, so no build has been uploaded, to TestFlight or to a Play track |
| M12 | **Screen-reader traversal** — VoiceOver/TalkBack can navigate the phone shell and the read-only plan screen | **user** | Needs a device with a screen reader; the axe runs in `app/src/features/a11y.test.tsx` are a floor and say nothing about traversal | **NOT DONE.** axe passing is not a traversal; the keyboard and focus behaviour is asserted per screen, but a screen reader on a phone has not been run |

**`scripts/smoke-phase14.sh` runs the machine half of this table** — steps 1 and 2 are M9's
evidence, step 3 is M10's gate 5, and step 4 carries gate 6's case. The rows above that it
cannot reach are the ones typed `user` or `blocked`, and the script prints them by name
rather than reporting a pass for a device that was never plugged in.

**The smoke run's exit code was 1, and the one failing step belongs to Phase 13.** Steps 1,
2 and 4 passed — stamp `store` with **0** occurrences of `billing/checkout` and `no purchase
UI in 3 shipped file(s)`; `no third-party loads in 3 shipped file(s)`; and **600 passed, 0
failed** across 65 files, gate 6's Indonesian phone-shell case among them. Step 3 failed on
**22 clipped elements, all at 200% text on Phase 13's five checkout states** — the same
defect M10 records as Phase 13's rather than this phase's. An earlier run of the same script
exited 3 because concurrent slices were mid-edit (a broken `settings/AccountData.test.tsx`
stopped the build, and `SettingsScreen.tsx` added 42 `settings/dark/x200` overflows); step 1
was hardened to empty `app/dist` first, because every slice builds into that directory and a
`full` build landing there is read as part of the store bundle.

## Store policy risk (the Phase 13 position, confirmed)

Billing is QRIS only, and **the iOS and Android apps do not sell anything**: no
checkout, no purchase buttons, no prices, and no links or instructions pointing
to a website purchase. Users buy on the web or the desktop app, and the plan then
syncs to their account on every device.

The three enforcement points are **live**, not planned:

1. `config/billing_policy.toml`. `validate()` rejects a mobile entry that
   enables purchase UI, prices, or an upgrade CTA, so the file cannot be edited
   into a violation.
2. `GET /plans` omits prices where `show_prices` is false, including the
   credit-pack prices, so a phone is not told a price it must not show.
3. `POST /billing/checkout`, `POST /billing/upgrade` and `POST /billing/downgrade`
   are refused for `platform in (ios, android)` with 403 `feature_locked`, and the
   platform is read from `devices.platform` rather than from the request body
   (A144) — so a patched client that claims `web` from an iPhone is still refused.

A fourth enforcement point is the **build** rather than the server, and it is the
one that makes the first three more than a rendering decision: the store build is
keyed on `TAURI_ENV_PLATFORM`, the router's purchase routes and `BillingScreen` are
guarded by `__PURCHASE_UI__`, and `scripts/check-no-purchase-ui.mjs` refuses an
artifact that does not carry the `stvdy-build-target="store"` stamp. The route
table in a store build has no `/pricing` and no `/billing/checkout/:paymentId` at
all, so there is no dead link to a screen that is not there.

**The residual risk is accepted, with its trigger.** Both Apple and Google require
their own in-app purchase for digital subscriptions in many regions, and a
QRIS-only mobile app that unlocks paid features may be rejected or required to add
store billing. The `PaymentProvider` trait is kept generic so store billing can be
added later without restructuring. **The trigger is the confirmation step below:**
the risk is accepted *against the current Indonesian rules as re-read on the
submission date*, not accepted once and forever. If step 1 or step 2 comes back
"required", this section is wrong and the store-billing work becomes a phase rather
than a note.

### The confirmation step, and the date

This note is the one part of this document that **can expire**, so it is a step
with a date rather than a paragraph. Before each store submission:

| # | Step | Date | Result |
|---|---|---|---|
| 1 | Re-read Apple's App Review Guidelines § 3.1.1 and § 3.1.3 (In-App Purchase, and the reader/multiplatform exception) for the current Indonesian position | `<fill on the submission date>` | `<required / not required / exception applies>` |
| 2 | Re-read Google Play's Payments policy § "Payments" for Indonesia, and its current list of permitted alternative billing programmes | `<fill>` | `<required / not required>` |
| 3 | Confirm the *residual* risk this leaves: a reviewer who reaches the **web** checkout — which is a normal, correct part of the product on a desktop browser — could read it as an off-store purchase path being advertised to a mobile user | `<fill>` | `<accepted, with the evidence in step 4>` |
| 4 | Point at the evidence that answers step 3, and record its result | `<fill>` | `scripts/check-no-purchase-ui.mjs` and `scripts/check-mobile-no-purchase.sh`, both green on the submission commit |

Step 3 is the risk in its real shape, and it is worth stating precisely because
the mitigation is not a policy edit — it is the *absence of the affordance in the
shipped bytes*. A mobile build that contains no price, no checkout call, no
upgrade CTA and no link to a web checkout cannot advertise one to a reviewer,
whatever the website at large does.

**Where that evidence lives.** Three places, and the third is the one a reviewer's
question actually turns on:

1. `scripts/check-no-purchase-ui.mjs` — the check itself. It holds the purchase
   denylist (a readable rupiah price, `billing/checkout`, a `buy(`/`checkout(`/
   `purchase(` call), the rule that a scan of **nothing** is a failure rather
   than a pass, and a second assertion that `config/billing_policy.toml` still
   sets `allow_purchase_ui`, `show_prices` and
   `allow_purchase_cta_in_locked_messages` to `false` on `ios` and `android`.
   Its own tests are `scripts/check-no-purchase-ui.test.mjs` (nine cases,
   including the empty-bundle case and the strings the app legitimately ships).
2. **CI runs it on every commit**, in the `web` job, *after* `npm run build`
   (`.github/workflows/ci.yml`): a check that is only ever run by hand is the one
   that gets skipped. The same job runs `scripts/check-no-cdn.mjs`, so a build
   cannot satisfy one property by breaking the other.
3. `scripts/check-mobile-no-purchase.sh` — the **release-time** wrapper, run
   against the artifact rather than against `app/dist`. It runs the same check
   (it does not reimplement it) against the web assets a packaged build ships,
   and it reports how many files it inspected. It fails on a hit, on an empty
   directory, and — as exit code 2 — when it *could not check at all*, because
   "could not check" and "clean" are different answers.

The server is the third layer and is not in the bundle: `POST /billing/checkout`
is refused for `platform in (ios, android)` (A24, `crates/server/src/billing/mod.rs`),
and `GET /plans` omits prices where `show_prices` is false
(`crates/server/src/billing/routes.rs`).

## Store submission checklists

Per store, per artifact, with the evidence named (A351). **A row without a command
or an owner is a wish**, so every row below is one of:

- **code** — closed by this repository, and the command that proves it is in the row.
- **user** — a credential, an account, a legal review or a host decision. The user
  supplies it; no agent can. `docs/superpowers/HANDOVER.md` is the live list of what
  the user owes and is the authority for those rows, so this section names the row
  and does not restate it.
- **blocked** — waiting on something nobody has, with the workaround that is live
  until it lands.

**Two artifacts, not one.** The Play build and the direct-APK build carry the same
version string and the same `config/billing_policy.toml`, and they are signed by
**different keys** (A352). Run the checklists against each artifact you actually
upload, and record which key signed it.

### Apple — App Store / TestFlight

| # | Item | Kind | Evidence / owner |
|---|---|---|---|
| A1 | **Privacy labels** — every data class in the EULA's inventory, each with its purpose and whether it is linked to identity | **user** | `docs/EULA.md` § 9 ("What we collect, why, and for how long") is the inventory; the labels are App Store Connect fields and the wording is B2's (`HANDOVER.md` B2) |
| A2 | **In-app account deletion**, reachable from inside the app, with the consequence stated *before* confirmation | **code** | Shipped by A355. The screen is `app/src/features/settings/SettingsScreen.tsx`'s `DataSection` — a type-to-confirm step with the forfeiture stated above the field — and the server is `DELETE /account` in `crates/server/src/account.rs`. The consequence copy matches `docs/EULA.md` § 5. The proof is `crates/server/tests/account.rs` (8 cases, including that a signed-in session is refused *before* the purge) and `app/src/features/settings/AccountData.test.tsx` (6 cases). Apple requires in-app deletion of an account an app can create |
| A3 | **Review notes** — QRIS-only billing, that the app sells nothing, and where a reviewer creates an account and sees the read-only plan screen | **code** | The plan screen is `app/src/features/billing/MobilePlanScreen.tsx` (A322). The notes themselves are text the user submits |
| A4 | **The build sells nothing** — no price, no checkout, no upgrade CTA, no link to a web checkout | **code** | `scripts/check-mobile-no-purchase.sh <artifact.ipa>` (exit 0), and `node scripts/check-no-purchase-ui.mjs` on the same commit — **against the store build**. The check refuses to pass on anything not stamped `stvdy-build-target="store"`, so `app/dist` must hold the store bundle: `TAURI_ENV_PLATFORM=ios npm --prefix app run build` (what `tauri build` itself runs on a phone), or `npm --prefix app run build:mobile` for `dist-mobile`. A plain `npm --prefix app run build` produces the web/desktop build, which the check rejects by design |
| A5 | **Build number** — `CFBundleVersion` bumps on every submission | **code** | `bundle.iOS.bundleVersion` and `bundle.macOS.bundleVersion` in `src-tauri/tauri.conf.json` (both `"1"`), which the Tauri CLI's own schema defines per platform as "translates to CFBundleVersion". Bump **those** before a submission; the shell version is the short version (`CFBundleShortVersionString`) and is a separate question (A353) | **Ready.** The key is per-platform, not top-level — `bundle.bundleVersion` is not in the schema's `BundleConfig` at all, so the top-level form would be rejected or silently ignored |
| A6 | **Apple Developer Program membership + Developer ID certificate**, and the notarization credential | **user** | `docs/superpowers/HANDOVER.md` § Credentials (Phase 15); A333 |
| A7 | **TestFlight internal testing** on a physical device before review | **user** | Needs the account in A6 and a device; `HANDOVER.md` (Phase 14) records that the push and background cases need real hardware |

### Google — Play (internal testing, then production)

| # | Item | Kind | Evidence / owner |
|---|---|---|---|
| G1 | **Data Safety form** — the same EULA inventory mapped to Google's categories | **user** | `docs/EULA.md` § 9; the mapping and the wording are B2's (`HANDOVER.md` B2) |
| G2 | **Account-deletion URL** — a public web page where a user can request deletion without installing the app | **user** | The *in-app* half now exists (A355, see A2): `DELETE /account` marks the account, revokes access and schedules the purge. What is missing is only the **public page** — it is a deployment artefact, not code, and it needs the production host (launch checklist row 12). Play requires the URL for any app with account creation, and the page must describe the same window the code implements (`ACCOUNT_DELETION_GRACE_DAYS`, 7 by default) |
| G3 | **Review notes** — QRIS-only, no purchase UI, how to reach the read-only plan screen | **code** | Same as A3 |
| G4 | **The build sells nothing** | **code** | `scripts/check-mobile-no-purchase.sh <artifact.apk>` (exit 0) |
| G5 | **Internal testing track before production** | **user** | Play Console; `HANDOVER.md` § Credentials (Phase 14) for the account |
| G6 | **Play App Signing key vs the direct-APK key** — different keys, recorded per artifact | **user** | A352. Confusing them is how an upgrade fails to install: the direct APK must be signed by the operator's key, and Play re-signs the Play build with its own |
| G7 | **Google Play Console account** | **user** | `docs/superpowers/HANDOVER.md` § Credentials (Phase 14) |

### What the absence check inspects, and what it does not

**What it does:** the text files a mobile build ships to the webview — `.html`,
`.js`, `.css`, `.json`, `.svg` — under `app/dist` and, for an archive target,
under every extracted member. It also re-asserts `config/billing_policy.toml`.

**What it does not, and no part of its output should be read as covering:** the
native layer (`lib*.so`, `classes.dex`, `Payload/*.app` binaries), the store
metadata and screenshots, the review notes, or whether the account-deletion
screen behaves as described. Rows A2, G2 and A1/G1 are exactly the ones it cannot
answer.

**It cannot be exercised against a real mobile artifact on this machine**, and
that is stated rather than implied: there is no Android SDK (`~/Library/Android/sdk`
is absent) and no signed iOS build, so no `.apk` or `.ipa` exists to point it at.
The CI-only path is `scripts/check-mobile-no-purchase.sh` with no argument, which
checks `app/dist` — the tree every mobile build packages. What *has* been
exercised is the script's failure modes: a hit (exit 1), an empty directory
(exit 1), an artifact with no web assets at all (exit 1), an uncheckable
argument (exit 2), and a clean tree (exit 0).

## Launch checklist

**Every row is `code`, `user` or `blocked`, and the checklist cannot be ticked by
an agent.** That is the point: it is the one place where "I did the reachable
part" and "the product can launch" are visibly different sentences. Rows 1-7 are
this phase's; rows 8-16 are not, and a plan that marked them done would be lying
about the only thing that matters at launch.

`docs/superpowers/HANDOVER.md` is the live list of what the user owes; the rows
below name it rather than repeating it, so the two cannot drift.

| # | Item | Kind | Evidence / owner |
|---|---|---|---|
| 1 | Every phase's exit gate green on the release commit | **code** | the `ci.yml` run URL for the release commit |
| 2 | macOS signed + notarized + stapled | **code** | `scripts/verify-release-artifacts.sh --macos` |
| 3 | Windows installer signed | **code** | `scripts/verify-release-artifacts.sh --windows` |
| 4 | Updater manifest signed and published | **code** | `scripts/verify-release-artifacts.sh --manifest` |
| 5 | Image built and the PDF test passes inside it | **code** | `scripts/release-image.sh` (builds, boots against Postgres, runs the PDF acceptance test inside the container, compares the running Chromium to the pin) + the `image` job in `ci.yml`. `verify-release-artifacts.sh --all` is **not** this row: `--all` is version + macos + windows + manifest |
| 6 | Store checklists complete | **code** | § Store submission checklists above |
| 7 | Mobile bundle has no purchase UI | **code** | `scripts/check-mobile-no-purchase.sh` (exit 0, with the file count in its output) |
| 8 | EULA/privacy reviewed and published | **user** | `docs/EULA.md` is a marked draft; legal review, `HANDOVER.md` B2 |
| 9 | Tax/PPN treatment settled, invoices carry the line items | **user** | B2 (`HANDOVER.md`); invoices currently carry no tax lines |
| 10 | SMTP credentials set; a real verification email arrives | **user** | B3 (`HANDOVER.md`) |
| 11 | QRIS gateway integrated against its real docs | **blocked** | B1 (`HANDOVER.md`); Phase 13 is implemented against an env-driven internal contract |
| 12 | Production host provisioned and `/health` green there | **user** | A330, A45 (`HANDOVER.md`); the Batam VPS is a 1-month trial and is staging only |
| 13 | R2 bucket live and a presigned URL works | **user** | A15/A331 (`HANDOVER.md`) |
| 14 | Restore drill run against the production backup | **user** | `scripts/restore-drill.sh` exists and runs; running it against **production** needs the host in row 12, so the production claim is the user's |
| 15 | Uptime check and alert routing live | **user** | A348; `docs/OPERATIONS.md` carries the four signals, the routing is the operator's |
| 16 | Breach runbook's two blanks filled in | **user** | A349; "who decides" and "how long we have" are blanks on purpose |

**Two rows this slice added, because they are store blockers and were not in the
plan's table:** A2/G2 (in-app account deletion and the deletion URL) and A5 (the mobile
build number had no `bundleVersion` to bump). **Both have since moved:** A2's code half shipped
(A355), so the in-app row is now `code` and only the public URL remains `user` — it needs the
production host in row 12. **A5 has since been fixed too** — `bundle.iOS.bundleVersion` and
`bundle.macOS.bundleVersion` are both `"1"`, so the row below now names the keys to bump rather
than describing the gap.

## Privacy labels

Store privacy disclosures must be confirmed before launch, and **the inventory is
`docs/EULA.md` § 9** ("What we collect, why, and for how long") — it is listed once, there, so the
notice the user reads and the labels the stores require cannot drift into two different claims.
This section deliberately does not repeat the list.

The two forms are App Store Connect's privacy labels and Play's Data Safety form, and **both are
typed `user`** in § Store submission checklists (A1, G1): the data classes are known and written
down, but the wording and the mapping to each store's categories are B2's legal input. A class the
product collects and the form omits is a false statement to the store, which is why the rows are
not closed by an agent.

## Account deletion

**In-app `Delete account` is required by app store rules, and it is shipped (A355).**

- It deletes or anonymizes the user's data, revokes all sessions, and cancels pending jobs.
- There are no recurring charges to cancel. **Remaining prepaid time is forfeited, and that is
  stated in the UI before confirmation** — `SettingsScreen.tsx`'s data section says so above the
  type-to-confirm field, and `docs/EULA.md` § 5 is the promise it keeps.
- `Export my data` produces JSON + Markdown + ICS.

**What exists:** `DELETE /account` and `GET /account/export` in `crates/server/src/account.rs`,
merged into the authenticated routes; the `purge_deleted_accounts` handler in
`crates/server/src/jobs/handlers/purge_accounts.rs`, dispatched by the worker and enqueued by the
scheduler's `pass` as a backstop; `migrations/0037_account_purge.sql` (`users.purged_at`); the
settings screen's data section; and `app/src/api/queries/account.ts`.

**The window is 7 days (`ACCOUNT_DELETION_GRACE_DAYS`, A355), and it is the purge's scheduling
rather than a recoverable state.** The deletion is effective at the request: sessions, devices,
the ICS feed token and pending credentials are revoked immediately, jobs are cancelled, and login
is refused with `403 account_deleted` naming the purge date. What the purge deletes and what it
keeps is A355a's per-table-group mapping — the student's content goes, and `payments`,
`credit_ledger`, `subscriptions` (cancelled), `usage_events` and `change_log` survive attached to
an anonymized `users` row, which is what keeps the margin report and the tax position intact.

**Apple's in-app row is `code` (A2) and Play's public deletion URL is `user` (G2).** The URL is a
deployment artefact rather than code and it needs the production host (launch checklist row 12);
the page must state the same window the code implements. The remaining gap is that page, not the
feature.

## Release process

The sequence, with the commands. **A release is a tag, not a branch**: tags are not commits, and a
pipeline that trusts a green badge from a branch can ship a commit CI never saw (A345).

### 1. Land everything, and let CI be green on the commit

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
just check-shell                    # src-tauri is outside the workspace, so this is separate
npm --prefix app test
npm --prefix app run build
node scripts/check-no-cdn.mjs
node scripts/check-no-purchase-ui.mjs
node scripts/check-no-purchase-ui.test.mjs
bash scripts/check-mobile-no-purchase.sh
```

`check-shell` depends on `check-web` on purpose: `generate_context!()` panics at compile time when
`app/dist` is missing, and `dist/` is gitignored.

### 2. Bump the version, in every place that must agree

`Cargo.toml`'s `[workspace.package] version`, `src-tauri/Cargo.toml`,
`src-tauri/tauri.conf.json`, both blocks of `src-tauri/gen/apple/project.yml.template` and the
generated `project.yml` beside it, and the two lock files — the table in § Versioning is the
list, with the test or the workflow step that binds each one. A bump that misses
`tauri.conf.json` or the shell's manifest fails
`crates/server/tests/release.rs::the_version_is_the_workspace_version`; one that misses the iOS
template fails `src-tauri/tests/share_group.rs::the_extension_version_matches_the_apps` on CI,
which is how v0.1.1's was caught after the tag.

### 3. Tag, and push the tag

```bash
git tag v0.2.0            # the shell's version; `server-v…` for a server-only hotfix
git push origin v0.2.0
```

`.github/workflows/release.yml` triggers on `v*`. Its **first job** re-runs the step-1 checks on
the tagged commit; a failure there stops the release before anything is built. Then three build
jobs (macOS universal, Windows NSIS, the Android APK for the direct channel), each uploading its
artifacts **and its `.sig` files** — a job that produced an archive and not its signature is a
failure, and the workflow asserts the `.sig` exists rather than trusting the bundler's exit code.

### 4. The manifest, and the round trip

`latest.json` is generated with one entry per `{{target}}/{{arch}}` pair, each carrying the
`signature` (the contents of the `.sig`) and the artifact's `url`, and it is signed with the same
minisign key. The workflow then **downloads the published manifest back over HTTPS and verifies
its signature against the committed public key**, failing if that round trip does not hold — the
manifest is the artifact the whole channel rests on.

### 5. Verify the artifacts before publishing

The last step of every job runs the gate against what it just produced:

```bash
scripts/verify-release-artifacts.sh --macos   --dir <bundle dir>
scripts/verify-release-artifacts.sh --windows --dir <bundle dir>
scripts/verify-release-artifacts.sh --manifest --manifest-dir <dist dir>
scripts/verify-release-artifacts.sh --version --tag v0.2.0
```

**This is the step that makes the exit gate mechanical rather than a claim**: a job that produced
an unsigned installer or an unstapled dmg fails here, before the artifact is published. Every mode
is fail-closed — a check that cannot read what it needs is a `FAIL`, never a skip — and the script
**refuses to run at all (exit 2) when `TAURI_SIGNING_PRIVATE_KEY` or its password is in its
environment**, because a verification tool that can sign cannot establish the one property it
exists to establish (A332).

> **The consequence for the workflow, stated so it is not discovered by a failing release:** the
> signing secret must be scoped to the signing **step**, never to the job. A job-wide secret is how
> the key ends up in the environment of every later step — which is exactly the drift A332 forbids,
> and the verifier refuses rather than tolerating it.

### 6. Publish, and record

Attach the artifacts to the GitHub Release, publish `latest.json` and its `.sig` to the static
endpoint `plugins.updater.endpoints` names, and record the run URL in the launch checklist
(`docs/RELEASE.md` § Launch checklist). Then deploy the image — § Server deployment below.

### What degrades without a certificate, and what that costs

Every row is the user's to obtain; `docs/superpowers/HANDOVER.md` is the live list, and this
document does not restate it.

| Missing | What still builds | What it costs |
|---|---|---|
| Apple Developer ID certificate | a universal `.app`/`.dmg`, unsigned | Runs **only** on the machine that built it (Gatekeeper trusts a locally built app) and is blocked on every other Mac. **Not distributable** |
| notarytool credentials | a signed but unstapled dmg | A first launch needs a network round trip to Apple; offline, it fails with the "cannot be opened" dialog |
| Windows Authenticode certificate | the NSIS installer, unsigned | SmartScreen warns on every install. The warning is per-publisher reputation, so it does not decay |
| a real minisign keypair | everything, with a placeholder `pubkey` | Every signature fails to verify — a safe-but-useless updater. The plugin requires `plugins.updater` to exist, so the honest alternative is no updater at all |
| a production host | every artifact | Nothing can be *deployed*. A45 forbids the trial VPS, and putting the manifest endpoint or the QRIS webhook behind it is the specific mistake that row names |

### The unsigned path, for a build that exists before the certificates do

`.github/workflows/unsigned-artifacts.yml` is `workflow_dispatch`-only and produces the three
installers **without** any certificate: a macOS universal `.dmg`, a Windows NSIS `.exe` and the
direct-channel `.apk`. It exists because `release.yml` refuses to publish an unsigned macOS build
on purpose (A333), and that refusal is a property of the signed channel rather than an
inconvenience in it — so the two are separate files, and this one says on every artifact what is
missing.

```bash
gh workflow run unsigned-artifacts.yml -f version=0.1.2 \
  -f visibility=prerelease -f apk_signing=operator-keystore
```

Three properties it holds, each for a reason:

- **It cannot reach the updater channel.** Every build overrides
  `bundle.createUpdaterArtifacts` to `false`, so no `latest.json` is produced or published and no
  installed app can be offered a build from here. The override is also load-bearing: at its
  committed value the CLI refuses to build at all without `TAURI_SIGNING_PRIVATE_KEY`.
- **The version is checked before anything is built**, against `Cargo.toml`,
  `tauri.conf.json` and `src-tauri/gen/apple/project.yml.template` — the four places a bump has
  to reach (see § Versioning).
- **The APK is signed by a named key.** `apk_signing=operator-keystore` needs the four
  `ANDROID_*` secrets and produces the direct-channel APK; `apk_signing=debug` needs no key
  material and produces a debug-signed APK that installs but **cannot** be updated in place by a
  later release build. The Release notes say which of the two a given file is.

It also **generates** the Android project (`tauri android init --ci`), because
`src-tauri/gen/android` is not committed in this tree — Phase 14's Android half was blocked
without an SDK. Two consequences worth knowing before Phase 14 lands:

- the generated project has **no signing configuration**, so this workflow appends one before
  building; a committed project must carry its own, or `release.yml`'s Android job will build an
  unsigned release APK and Gradle will refuse it;
- the NDK is selected through `NDK_HOME`, which is what `cargo-mobile2` reads
  (`src/android/ndk.rs::Env::new`) — it is not discovered from Gradle, and the generated project
  sets no `ndkVersion`.

**A build from this workflow is not a release.** It is not notarized, it is not signed on
Windows, and it is published as a pre-release or a draft; the store and direct-download
distribution paths remain the ones above.

## Server deployment

The image is the deployment unit: API, job worker and scheduler are three tasks in one process
(`crates/server/src/main.rs`), so one container is the whole server. `docs/OPERATIONS.md` is the
step-by-step version, and this section is the summary of what a release has to be true of.

**The environment.** `.env.example` is the contract. Production sets `APP_ENV=prod` (which requires
`JWT_SECRET` of at least 32 characters and a `WEB_ORIGIN`), `DATABASE_URL`, `ENCRYPTION_KEY_HEX`,
`STORAGE_BACKEND=s3` with the R2 values (`S3_ENDPOINT`, `S3_BUCKET`, `S3_REGION=auto`,
`S3_FORCE_PATH_STYLE=false`; A331), `SEARXNG_URL` pointing at the **separate** search service
(A344), and the image's own defaults for `CHROME_PATH`, `PDFIUM_LIB_PATH`, `FASTEMBED_CACHE_DIR`,
`SOFFICE_BIN` and `ASSETS_DIR` (A337-A343).

**The migration is a step, not a side effect.** `--migrate-only` runs to completion and exits
without binding a port; it runs **once, before any replica starts** (A346). That is a deployment
contract rather than a convenience: `main.rs` migrates on every boot with no lock of its own, so
two replicas starting together race, and the one that loses serves traffic against a schema it has
not verified.

```bash
docker run --rm --network host -e DATABASE_URL="$DATABASE_URL" stvdy-server:<tag> --migrate-only
docker run -d --name stvdy --shm-size=1g --tmpfs /tmp <env…> -p 8080:8080 stvdy-server:<tag>
```

`--shm-size` and a writable `/tmp` are both required by Chromium's sandbox (A338).

**The reverse proxy.** TLS termination, `WEB_ORIGIN` pointed at the web build's origin, **buffering
off on the SSE route** (the heartbeat exists precisely because proxies close idle streams), and
`GET /assets/` public — the requester is headless Chromium printing a document, and every file is a
vendored upstream release with no user data in it (A339).

**Object storage.** R2, presigned reads only, no public bucket: `GET /sources/:id/file` already
hands out short-lived scoped URLs, and a public bucket would make the presigning decorative (A331).

**Before deploying, run the image check on the host.** "Works in CI" and "works on this host" are
different claims, and the second is the one that ships:

```bash
IMAGE=stvdy-server:<tag> DATABASE_URL="$DATABASE_URL" bash scripts/release-image.sh
```

It compares the running Chromium to the `Dockerfile`'s pin, checks pdfium, LibreOffice and a
non-empty model cache, runs `--migrate-only`, boots the server, probes `/health` and the four
`/assets/` paths, and then runs the PDF acceptance test **inside the container** — the same test
CI's `image` job runs, against the image's own browser. A pass proves the print assets resolved,
because a missing KaTeX leaves every formula as literal LaTeX.

## The server image bakes a pinned Chromium

**There is now a `Dockerfile`** (Phase 15, Task 6), and it is the authority for what the image
contains; `docs/ARCHITECTURE.md` § Container image is the table, and `docs/OPERATIONS.md` is
how to run it. Phase 6 recorded the requirement before the image existed, and it is unchanged:

- **The image bakes a pinned Chromium and sets `CHROME_PATH` to it.**
  `crates/server/src/render/chrome.rs::find_executable` reads `CHROME_PATH` first and
  only then probes the usual install locations, so a deployment that names its build
  gets exactly that build. Without `CHROME_PATH` the server falls back to a probe
  list that is meant for a developer's laptop, and a runner or image that has no
  Chrome at all leaves `AppState.renderer` as `None`.
- **A missing browser is not a crash.** Without a renderer the server still starts,
  still generates cheatsheets (the blocks and the HTML are stored, and the document
  is `ready`), and answers 409 `conflict` on `GET /cheatsheets/:id/pdf` and 502
  `provider_error` on a report's `format=pdf` export. So the symptom of a missing
  Chromium in the image is **a PDF nobody can download**, not a failed deploy — which
  is exactly why it needs to be a documented image requirement rather than something
  a health check catches (A130).
- **The image bakes headless LibreOffice and sets `SOFFICE_BIN` to it.** Phase 7's
  DOCX and PPTX sources get a PDF rendition through
  `soffice --headless --convert-to pdf` (`crates/server/src/sources/parse.rs::preview_pdf`),
  stored under `sources.preview_key` and served by `GET /sources/:id/preview`, so the
  client opens one media type for every source kind. This follows A51's absent-binary
  policy: when `SOFFICE_BIN` is unset the source **still ingests** — the text and the
  slide numbers come out of the OOXML itself, with no converter — the preview is absent
  and one `tracing::info!` is logged; when it is set but the conversion fails, the source
  still ingests and a `tracing::warn!` is logged, so an unloadable converter is louder
  than an absent one (A158). A missing LibreOffice is therefore not a crash and not a
  failed ingest: it is a source with no preview, and the remedy is the image.
- **The version is pinned, and the pin is asserted.** `ARG CHROMIUM_VERSION` names a
  specific Debian package version (never the `chromium` meta-package and never `stable`),
  because the renderer's output is the document a student prints and the acceptance test
  asserts a specific page count and a footer read back from the renderer. CI's `image` job
  compares the version the running image reports (`chromium --version`) to that `ARG`, so a
  base-image change that silently moves Chromium **fails the build** rather than changing
  every PDF.
- **The base is `trixie`, not `bookworm`, and the floor is `ort-sys`.** The prebuilt ONNX
  Runtime that `ort-sys` links is built against glibc ≥ 2.38 (`__isoc23_strtoll`,
  `__isoc23_strtoull`, `_M_replace_cold`) while bookworm carries 2.36, and `ort-sys`
  `2.0.0-rc.13`'s `build/download/dist.tsv` carries exactly one distribution per target —
  so there is no lower-glibc build to switch to and the base image is what had to move. Both
  stages move together: a binary linked against trixie's glibc does not load on bookworm.
  The pin follows the base into `trixie-security`, which is where a browser's current
  version lives — trixie's main archive carries an older one (`150.0.7871.181-1~deb13u1`
  against the security archive's `154.0.8037.92-1~deb13u1`, read 2026-10-12).
- **CI runs the acceptance test, and runs it in the image too.** `.github/workflows/ci.yml`
  has two jobs: `pdf` runs
  `CHROME_PATH=/usr/bin/google-chrome cargo test -p stvdy-server --test pdf -- --ignored`
  on `ubuntu-24.04` (failing before the test if that path is not executable), and `image`
  builds the Dockerfile, boots it against Postgres and runs the same test **inside the
  container** against the image's own Chromium, pdfium and assets. The test is `#[ignore]`d
  by default so a laptop without a browser still passes `cargo test --workspace`; the
  second job is what keeps that from making it a test nobody runs. `scripts/release-image.sh`
  is the readable form of the same sequence, and it is what an operator runs on the
  production host before deploying — "works in CI" and "works on this host" are different
  claims.
- **The image runs as a non-root user with Chromium's sandbox enabled**, and needs
  `--shm-size` plus a writable `/tmp`. Running as root with `--no-sandbox` is explicitly not
  the answer (A338); the `docker run` stanza is in `docs/OPERATIONS.md`.

The fonts the template names (`Inter`, `JetBrains Mono`, and the KaTeX woff2 files) are
**vendored in the repository and served by the server**, not fetched at render time. The
template used to link them from a CDN while its own comment claimed the renderer rewrote
`ASSET_BASE` — and no code did, so a sealed container rendered every formula as literal `$…$`.
`GET /assets/{*path}` serves them (`crates/server/src/assets.rs`, embedded with `rust-embed`)
and `render::chrome::rewrite_asset_base` points the template's references at it before the
page is written, using `ASSETS_BASE_URL` (default `http://127.0.0.1:<BIND_ADDR port>`; A339).
The `data-math-ready` guard is unchanged and is still the gate: an unreachable asset leaves
`renderMathInElement` undefined, the template marks `unavailable`, and the render fails rather
than shipping a document full of LaTeX source (A340).


### The webhook URL needs a host with the product's lifetime

`POST /webhooks/qris` is a **public** route (its authorisation is the signature, not a session), so
its URL is registered with the gateway and is the gateway's retry target for every payment. It must
**not** be hosted on the staging VPS: A45 makes that host a one-month trial, and a webhook URL that
dies with the trial takes the callback path with it — a gateway retrying into a 502 is a payment the
student made and the plan that never activated. The same requirement applies to the Google OAuth
redirect URI and the updater manifests, for the same reason and in the same sentence: all three are
addresses a third party has been told about, and none of them can move without coordination.

The two mitigations that exist are not substitutes for a durable host. `poll_payment` recovers a
callback that was never delivered, and `reconcile_payments` flags the ones that disagree — both run
inside the server, so a deployment with no reachable webhook URL still activates payments, just more
slowly and with more `review` rows. `QRIS_WEBHOOK_IPS` is opt-in and reads `X-Forwarded-For`'s last
hop, so a deployment behind a load balancer needs its own extractor before that allowlist means
anything; the signature is required either way.

## Tax and PPN

**Unconfirmed, and it is B2 that blocks it** (`docs/superpowers/HANDOVER.md`). Invoices currently
carry no tax line items, so a receipt issued today is not a tax document — which is why this is a
launch blocker rather than a formatting issue, and why launch checklist row 9 is typed `user`.

**The same blocker covers the privacy text.** B2 is "tax/PPN treatment **and** store privacy-label
text" because both are legal questions rather than technical ones, and `docs/EULA.md` carries the
draft of both: its tax clause and the retention window for payment records are `<placeholders>` on
purpose, and its § 9 data inventory is what the App Store privacy labels and the Play Data Safety
form are derived from. A value invented in either place would be a false statement made to every
user, so the draft says what it does not know. Launch checklist row 8 is the privacy half.

`docs/EULA.md` is **not published** and its own banner says so: it requires legal review before a
single user sees it. The service is not launched publicly until rows 8 and 9 are closed.

## Versioning

`CARGO_PKG_VERSION` for the server and shell, the same value in
`tauri.conf.json`. Mobile build numbers are bumped per submission.

**A bump touches more than those three, and the ones it misses are the ones nothing used to
check.** As of v0.1.2 the places are:

| Place | Bound by |
|---|---|
| `Cargo.toml` `[workspace.package] version` | `scripts/verify-release-artifacts.sh --version`, and the release workflow's `version` job |
| `src-tauri/tauri.conf.json` `version` | `crates/server/tests/release.rs::the_version_is_the_workspace_version` |
| `src-tauri/Cargo.toml` `version` | the same test |
| `src-tauri/gen/apple/project.yml.template`, both blocks | `src-tauri/tests/share_group.rs::the_extension_version_matches_the_apps` |
| `src-tauri/gen/apple/project.yml`, both blocks | nothing — it is XcodeGen's committed output, so it is byte-compared to the template by hand |
| the two committed lock files (`Cargo.lock`, `src-tauri/Cargo.lock`) | `cargo metadata` rewrites them; the local packages' versions live there. `src-tauri/plugins/mobile/Cargo.lock` carries none, so it does not move |
| `README.md`'s `/health` line, `docs/API.md`'s Phase 0 section | nothing — a documented response body that disagrees with the response is a false example |

The iOS blocks' **`CFBundleVersion`** is deliberately not in that list: it is `"0.1.0"` and
`"1"`, it was already stale at v0.1.1, and `a96a31d` recorded why moving it would be a guess —
which of the two the CLI's `bundle.iOS.bundleVersion` merge reaches is a question two built
artifacts did not settle.

**The server and the shell are versioned independently, and the tag names which one ships**
(A353). They are separate binaries with separate release cadences, and clients are not
recallable — so a server-only fix must be able to ship alone, and an update prompt for a fix
that does not touch the client is noise.

| Artifact | Versioned by | Reported by | Shipped by |
|---|---|---|---|
| The **shell** (desktop app) | `src-tauri/tauri.conf.json` `version`, plus `CARGO_PKG_VERSION` of `src-tauri` | the bundle's own metadata; the Settings → Updates panel shows what the manifest said | a `v*` tag, through the macOS/Windows jobs |
| The **server** | the workspace `Cargo.toml`'s `[workspace.package] version` | `GET /health`'s `version` field | the image, from the same repository at its own cadence |
| **Mobile** | the same shell version, plus a build number bumped per submission | the store | TestFlight / Play — **not** the Tauri updater |

So a release tag names the shell, and a server-only hotfix is tagged `server-v…` and skips the
client jobs. The workflow asserts the three values agree and fails with all three printed, and
`scripts/verify-release-artifacts.sh --version` is the same check on demand — it accepts a
`server-v…` tag against the workspace version **alone**, because the shell is not part of that
release. `crates/server/tests/release.rs::the_version_is_the_workspace_version` binds
`tauri.conf.json`'s version to the workspace version, so a bump that misses one fails the suite
rather than the release.

**What a hotfix may ship alone: the server.** The client cannot be recalled, and a client release
for a server-only fix asks every installed copy to download and reinstall for no change. The
reverse is not true: a client fix needs a release, and an installed copy gets it only by accepting
the update the manifest offers.

## Day-two operations

Backups and the restore drill, monitoring and alerting, the breach runbook, retention windows,
the production host checklist and the step-by-step deploy sequence are all in
**`docs/OPERATIONS.md`** (A354). They are there rather than here because this document is the
release's, and that one is the operator's — during an incident the shorter document is the one
that gets read.

Three things in it are launch prerequisites rather than nice-to-haves, and the launch checklist
rows name them: the restore drill (row 14), the uptime check and alert routing (row 15), and the
breach runbook's two blanks (row 16).
