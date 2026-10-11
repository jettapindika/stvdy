# Architecture

Filled incrementally; each section names the phase that completes it.

## Layers

```
app/  (one React SPA: web build + Tauri frontend for macOS/Windows/iOS/Android)
  |  HTTP + SSE
crates/server  (Axum: routes, auth, sync, job worker, integrations)
  |  traits
crates/core    (pure logic: chunking, retrieval, citations, SM-2, planner,
                entitlements, prompt composer, document renderer)
crates/sync    (wire DTOs shared by server and the native shell)
src-tauri      (native shell: keychain, local cache, plugins)
```

`crates/core` has no web, database or IO dependency, so its logic is unit-testable
against a fixed clock. That is what makes the planner, SM-2 replay and the
entitlements engine deterministic and cheap to test.

## Provider traits

All external capabilities sit behind traits in `crates/core/src/traits/`, so a
provider is configuration rather than code. Phase 0 defines the contract; each
phase adds its first implementation.

| Trait | First implementation | Phase |
|---|---|---|
| `LlmProvider` | OpenAI-compatible `/chat/completions` (streaming + tools) | 2 |
| `Embedder` | `fastembed` `MultilingualE5Base`, 768 dims, `query: `/`passage: ` prefixes applied by the impl (or an HTTP embedder) | 1 |
| `SearchProvider` | Tavily | 3 |
| `PaymentProvider` | `qris_gateway` | 13 |
| `PushProvider` | APNs (token auth) + FCM v1 | 12 |
| `Storage` | local filesystem (`STORAGE_BACKEND=local`, signed URLs served by this server) or S3-compatible (`aws-sdk-s3`; MinIO fork in dev, Cloudflare R2 in prod per A15) | 1 |
| `Mailer` | `lettre` over SMTP, or a dev file sink writing `var/mail/*.eml` | 1 |
| `Clock` | system clock; fixed clock in tests | 1 |

## Job worker

One in-process worker per server process, backed by a single `jobs` table (A10:
no Redis). An enqueue wakes the worker through a `Notify`; a 5-second poll
covers `run_at` scheduling and a delayed retry.

The claim is a single statement:

```sql
update jobs set status = 'running', attempts = attempts + 1
 where id = (select id from jobs
              where status = 'queued' and run_at <= now()
              order by run_at
              for update skip locked
              limit 1)
returning id, kind, payload, attempts;
```

`SKIP LOCKED` is what lets several workers, or several instances, run without
contending on the same row.

A failure is classified, not just reported. `Failure::transient` (a provider
hiccup, a timeout, a storage error) is retried up to `MAX_ATTEMPTS = 5` with a
`2^attempts`-second backoff, capped at `attempts.min(10)` so a long-lived row
cannot schedule a retry arbitrarily far out. `Failure::permanent` (a missing
pdfium library, an unparseable document, an unsupported source kind, an unknown
job kind) marks the job `failed` immediately. Retrying a permanent failure
leaves the source `processing` for half a minute and burns the queue behind it;
the stored `error` is what the client shows, so the message must be actionable.
A handler that panics is caught (`catch_unwind`) and treated as transient, so a
job row can never be left `running` by a panicking handler.

A boot sweep (`requeue_orphans`) requeues any row still `running`, because a
process killed mid-job would otherwise park that row and its source forever.

`dedupe_key` is a nullable column with a **partial** unique index
(`where dedupe_key is not null`). It exists so singleton recurring jobs
(`monthly_grant`, `compact_change_log`, `poll_payment`) can be enqueued from a
boot path and a duplicate is a no-op rather than a double-run. The consequence
to remember is on the writer: `on conflict (dedupe_key) where dedupe_key is not
null do update ...` must repeat the `WHERE` predicate, because Postgres will not
infer a partial index as a conflict target without it.

## Config

- `config/plans.toml`. Plans, limits, credit costs, provider cost ceilings.
  Loaded and validated at startup; `plans.toml` is the authority for
  entitlements, and the `plans` table is a mirror for FK integrity and auditing.
- `config/billing_policy.toml`. Per-platform purchase policy. `validate()`
  rejects a mobile entry that enables purchase UI, prices, or a CTA, so the file
  cannot be edited into an app-store policy violation.

## Trust boundary

**The client is untrusted; the server is the only enforcement point.** Everything
left of the API, the Tauri shell, the web SPA, and a scripted client holding a
valid token, is assumed hostile and possibly patched. The only thing a client
contributes is *authenticated intent* ("this user wants to send this message in
this workspace"); whether that intent is permitted, affordable, or well-formed is
decided on the server, per handler and per sync mutation.

`docs/THREAT_MODEL.md` is the normative statement of this boundary: the assets, the
attacker classes, what a modified client can and cannot do, the accepted residual
risks, tamper resistance by platform, and the per-phase verification obligations
that follow from it. It is linked here and from `README.md` so it cannot drift out
of the document set, and the phases that implement against it are Phases 7
(`tamper.rs`), 8 (cross-tenant isolation) and 13 (payment trust).

## Native capability matrix

See `docs/research/2026-10-11-tauri-2-capability-matrix.md` for the verified
platform matrix, the items needing custom Swift/Kotlin, and the items that are
impossible (background JS execution, guaranteed periodic background sync).

Decisions recorded from that research:

- **Secure storage**: `keyring-core` 1.0.0 plus a per-platform store wired by
  `cfg`. The `keyring` crate's defaults are desktop-only, so iOS/Android need
  their own store crates. Android needs **no** Kotlin shim: android-activity
  already initialises `ndk-context`, and a second call aborts
  (`assert!(previous.is_none())`).
- **Push**: no official Tauri push plugin exists; the official notification
  plugin cannot receive APNs/FCM. Phase 14 owns this.
- **Scheduling**: the official notification plugin schedules local notifications
  that fire while the app is closed and offline on mobile, but ignores scheduling
  entirely on desktop. Desktop reminders therefore come from server push and
  in-app delivery, with an in-process timer while the app runs.
- **Updates**: `tauri-plugin-updater` needs a signing keypair and a release
  endpoint, so it is registered in Phase 15 rather than Phase 0. Registering it
  with placeholder config panics at startup.

## Native prototypes

### Phase 0. Keychain round trip (macOS): PASS

`src-tauri/src/keychain.rs` has a test that stores, reads, overwrites and
deletes a refresh token through the OS keychain. Result on macOS 27 (arm64):

```
test keychain::tests::keychain_round_trips_a_refresh_token ... ok
```

No keychain prompt appeared for the locally built binary. The shell also builds
and launches: `tauri build --debug --no-bundle` produces
`src-tauri/target/debug/stvdy-shell`, which runs and holds a window.

### Phase 1. Auth, devices, ingestion

The native half of Phase 1 is the deep-link round trip and keychain token
storage; the server half (auth, devices, workspaces, sources, ingestion) is
covered by `docs/API.md` and the provider traits above. The acceptance question
was "does a `stvdy://` URL reach the SPA, on both cold start and a running
instance, and does the SPA get the refresh token into the OS keychain through the
adapter". Answer: the URL reaches the shell's classified intent on both cold
start and a running instance on macOS (PASS); the classification and adapter
halves of the keychain path are PASS, and the webview-to-Rust IPC hop was not
observed end to end (see the keychain line). The iOS simulator build is BLOCKED
and is recorded as such, not upgraded.

`src-tauri/src/deeplink.rs` holds the pure classifier: `stvdy://auth/callback`
with a non-blank `code` and `state` becomes an `auth_callback` intent,
`stvdy://oauth/calendar` becomes a `calendar` intent, and everything else
(foreign scheme, unknown route, missing or blank `code`/`state`) becomes an
`ignored` intent carrying a coarse reason. It never panics, and it logs only the
action label, never the URL, because a query string can carry an OAuth `code`.
The shell emits `{action, payload}` on the `stvdy://deep-link-intent` event and
answers `deep_link_current` with the same shape, so no module outside
`app/src/platform/` parses a URL. `DeepLinksApi.current`/`onOpen` in
`app/src/platform/types.ts` now take `DeepLinkIntent` instead of `string`.

#### Item 1. Deep-link classification unit tests: PASS

```
cargo test --manifest-path src-tauri/Cargo.toml
```

```
running 12 tests
test deeplink::tests::a_foreign_scheme_is_ignored ... ok
test deeplink::tests::an_ignored_intent_never_carries_the_url ... ok
test deeplink::tests::an_unknown_route_is_ignored ... ok
test deeplink::tests::classifies_a_calendar_return ... ok
test deeplink::tests::classifies_an_auth_callback ... ok
test deeplink::tests::malformed_input_never_panics ... ok
test deeplink::tests::missing_or_blank_credentials_are_ignored_with_a_reason ... ok
test deeplink::tests::percent_decodes_the_code_and_state ... ok
test deeplink::tests::serializes_to_the_action_payload_shape ... ok
test deeplink::tests::the_action_label_is_loggable_and_url_free ... ok
test keychain::tests::secure_store_availability_matches_the_platform_matrix ... ok
test keychain::tests::keychain_round_trips_a_refresh_token ... ok

test result: ok. 12 passed; 0 failed; 0 ignored
```

`an_ignored_intent_never_carries_the_url` asserts the serialized `ignored`
intent contains only `{"action":"ignored","payload":{"reason":"not_stvdy_scheme"}}`,
so a foreign link with a `code` cannot smuggle the credential into a log line.

#### Item 2. Running-instance deep link (macOS): PASS

```
./app/node_modules/.bin/tauri build --debug
open -a Stvdy.app --stdout /tmp/stvdy.run.out
open "stvdy://auth/callback?code=test&state=abc"
```

Observed on stdout (the shell was launched with `open --stdout`/`--stderr` to
capture its log):

```
2026-10-11T09:17:10.535811Z  INFO stvdy_shell_lib: deep link received action="auth_callback"
```

After the `stage` field was added to separate the two paths, a second run with
both routes:

```
open "stvdy://oauth/calendar?provider=google&state=abc"
open "stvdy://auth/callback?code=run1&state=s1"
```

```
2026-10-11T09:20:23.465115Z  INFO stvdy_shell_lib: deep link received action="calendar" stage="event"
2026-10-11T09:20:24.578812Z  INFO stvdy_shell_lib: deep link received action="auth_callback" stage="event"
```

#### Item 3. Cold start (macOS): PASS

```
pkill -f stvdy-shell; open -a Stvdy.app --stdout /tmp/stvdy.cold.out "stvdy://auth/callback?code=coldstart&state=xyz"
```

```
2026-10-11T09:19:26.531218Z  INFO stvdy_shell_lib: deep link received action="auth_callback" stage="launch"
```

This is the `stage="launch"` path, not `stage="event"`: on macOS the launch URL
reaches the deep-link plugin through `RunEvent::Opened` before the app's `setup`
hook runs, so the `deep-link://new-url` listener is not registered yet and the
URL only lands in the plugin's `current` slot. The shell classifies `current`
once during `setup` (for the log line) and exposes it through the
`deep_link_current` command, which the SPA reads on boot; the event covers a
running instance. Windows and Linux are the mirror image: there the URL arrives
as the second CLI argument, which the plugin reads at init, so `current` is
again the only delivery path.

#### Item 4. Ignored URLs and log hygiene: PASS

```
open "stvdy://nope/route"
open "https://example.com/cb?code=SECRET&state=x"
```

```
2026-10-11T09:20:39.043218Z  INFO stvdy_shell_lib: deep link received action="ignored" stage="event"
```

`grep -c SECRET` over the captured stdout returned `0`: the foreign URL's query
string is never logged. Note that macOS did not route the `https://` link to the
app at all (the shell is not registered for it), so this leg also exercises the
classifier's `not_stvdy_scheme` arm only through the unit test.

#### Item 5. SPA adapter contract: PASS

```
./app/node_modules/.bin/vitest run src/platform
```

```
 Test Files  3 passed (3)
      Tests  12 passed (12)
```

`app/src/platform/adapter.test.ts` pins that `current()` calls
`deep_link_current` and returns a classified intent (or null), that `onOpen`
listens on `stvdy://deep-link-intent` and forwards each intent with no extra
`forEach` arguments, and that mobile inherits the same path.

#### Keychain token storage (adapter -> `secret_set` -> OS keychain): PARTIAL

The path is: `platform.secureStore.set` -> `invoke('secret_set')` ->
`commands::secret_set` -> `keychain::set` -> the macOS keychain. Two of the three
hops are observed:

- Adapter to command: PASS. `adapter.test.ts` asserts `secureStore.set` calls
  `invoke('secret_set', { key, value })` and that `get`/`delete` call
  `secret_get`/`secret_delete`.
- Command to keychain: PASS. `keychain::tests::keychain_round_trips_a_refresh_token`
  stores, reads, overwrites and deletes a refresh token through the real macOS
  keychain (Phase 0 above, still green).
- The webview-to-Rust IPC hop in a live app was not observed: there is no way to
  drive the bundled webview's JavaScript from the command line on this machine,
  so the SPA-driven write was not exercised end to end. `security
  find-generic-password -s app.stvdy` reports no item, which is consistent with
  the round-trip test deleting what it writes and with no live write having run.

#### Item 6. iOS simulator build: BLOCKED

`./app/node_modules/.bin/tauri ios init` succeeded, so the build was attempted
per the plan:

```
./app/node_modules/.bin/tauri ios init
./app/node_modules/.bin/tauri ios build --target aarch64-sim --debug
```

`ios init` installed `libimobiledevice` via Homebrew, confirmed CocoaPods 1.16.2,
and generated `src-tauri/gen/apple/stvdy-shell.xcodeproj` (the `gen` tree is
tracked by design; see `.gitignore`). The build then failed before compiling:

```
Warn iOS platform not installed
iOS platform not installed
       Error iOS platform not installed
```

The blocker is a missing iOS simulator runtime, not the Rust side: Xcode 27.0 is
installed and the Rust targets `aarch64-apple-ios`, `aarch64-apple-ios-sim` and
`x86_64-apple-ios` are present, but no simulator platform is downloaded:

```
xcrun simctl list runtimes      -> (empty)
xcrun simctl list devices available -> (empty)
```

The Tauri CLI's message sits next to `simctl` and "Would you like to install the
latest iOS runtime? -downloadPlatform", so it is checking for that runtime. Per
the task's instruction not to work around a missing toolchain piece, this stopped
here rather than downloading the runtime.

#### Incident. `beforeBuildCommand` blocked by a sibling's TypeScript error

The first `tauri build --debug` ran the configured `beforeBuildCommand`
(`npm run build`) and failed inside another agent's tree:

```
src/api/queries/sources.ts(121,10): error TS2304: Cannot find name 'query'.
beforeBuildCommand `npm run build` failed with exit code 1
```

The bundle was produced by overriding the pre-build step on the command line
(`--config '{"build":{"beforeBuildCommand":""}}'`) against the `app/dist` built
minutes earlier, with no file change. This is reported, not fixed: `app/src/api`
is outside this task's ownership.

### Mobile capability spike (roadmap Phase 1: "do not defer to Phase 14")

The roadmap requires the riskiest native pieces be prototyped in Phase 1, because
their failure forces a Swift/Kotlin plugin or a framework change. Every probe is
recorded here, pass and fail, with the prerequisite it is waiting on. Three of
them need hardware this machine does not have, and that is a user-side
prerequisite rather than a code change, so it is named rather than assumed.

| Probe | Result | Prerequisite it needs |
|---|---|---|
| APNs token registration (iOS) | **NOT ATTEMPTED** | A device build. The iOS simulator runtime is absent (`xcrun simctl list runtimes` is empty), and APNs registration is not meaningful on a simulator anyway, so a physical device plus a signing identity is required |
| FCM token registration (Android) | **NOT ATTEMPTED** | An Android SDK (`~/Library/Android/sdk` does not exist) plus a physical device; `google-services.json` and the Gradle plugin are unwired |
| Foreground push receipt and tap-to-open (both) | **NOT ATTEMPTED** | The same device prerequisite. Note the recorded Phase 0 finding: the official notification plugin cannot receive remote push, so this probe is expected to need a custom plugin, and the spike exists to confirm that before Phase 14 |
| iOS "Open in..." document intake | **NOT ATTEMPTED** | A device build. Cold-start delivery under the scene lifecycle is the specific risk |
| iOS share-sheet row | **NOT ATTEMPTED** | A device build plus a Share Extension and an App Group, which are custom Swift and an entitlement path to confirm early |
| Android `content://` share and import | **NOT ATTEMPTED** | An Android SDK and a device. The Phase 0 finding stands: `mimeType: "*/*"` never matches, so a concrete type is required |
| Keychain round trip (macOS) | **PASS** | none (Phase 0, still green) |
| `stvdy://` deep-link round trip (macOS, running instance) | **PASS** | none |
| `stvdy://` deep-link round trip (macOS, cold start) | **PASS** | none |
| Foreign scheme ignored, no credential logged | **PASS** | none |
| Keychain adapter to command to OS keychain | **PARTIAL** | The live webview-to-Rust IPC hop needs a scriptable webview; both other hops are verified |
| iOS simulator build | **BLOCKED** | An installed iOS simulator runtime (see Item 6 above) |

**What is needed from the user:** an installed iOS simulator runtime plus a
physical iOS device (or a Mac with a signing identity) and an Android SDK with a
device, to answer the six NOT ATTEMPTED probes. Until then they are Phase 14
prerequisites rather than verified capabilities, which is the escalation the
roadmap's "or a framework change, which requires asking" anticipates.

### Accessibility gate (roadmap gate 5)

The SPA's screens for this phase (sign in, sign up, verify email, workspace list,
workspace detail with the upload and the source list) were checked against the
five requirements, in a real headless Chromium against the built bundle:

| Requirement | Result |
|---|---|
| Every control reachable by keyboard, with a visible focus ring | PASS. Tab-only walks on `/signin` and `/workspaces/:id` reach every control; each stop has a visible 2px solid ring, measured with `getComputedStyle(document.activeElement)` after real key presses rather than asserted from the stylesheet. The tablist is one tab stop with ArrowRight/Home/End moving both selection and focus |
| Contrast on the default dark **and** light themes | PASS for the screens above, at the token level: the design tokens are the single source of colour and were checked in both themes. The two defects this surfaced were fixed (a dark-mode card elevation and a citation hover contrast, commit 5ed5624) |
| Interactive targets >= 44px at phone width | PASS. The 44px label rows light up as one target (`has-[:focus-visible]`), including the radio group |
| No state conveyed by colour alone | PASS. Field errors carry `aria-invalid` plus `aria-describedby` text, and the upload's `pending`/`processing`/`ready`/`failed` states each render their own label rather than a colour swatch |
| 200% text scale without clipping or overlap | PASS on these screens (no fixed heights on text containers) |

Two further checks that this gate does not name but that the same walk exercised:
the delete-workspace confirmation moves focus to the confirming button (so a
keyboard user cannot confirm by accident), and cancelling it restores focus to the
trigger.

## CORS origins

`routes/mod.rs` gates CORS on `APP_ENV`: permissive in dev, an explicit allowlist
in prod. The non-obvious part is what belongs in that list. The Tauri webviews
are **not** `http://localhost`:

| Origin | Platform |
|---|---|
| `tauri://localhost` | macOS, iOS |
| `https://tauri.localhost` | Windows |
| `http://tauri.localhost` | Windows, Android |
| `WEB_ORIGIN` (env) | the deployed web build |

Omitting the `tauri.*` origins is the classic "works in dev, every desktop
request fails in prod" bug. They are compiled in; only `WEB_ORIGIN` varies per
deployment, and prod refuses to boot without it.

## Local cache

Native clients keep a SQLite cache plus an outbox, owned by the Rust shell
(`rusqlite`, `bundled`). Never by JS, so the sync engine and the database live
on the same side of the boundary. The web build has no local database and is
online-first. SQLCipher encryption is a config flag, off by default.
