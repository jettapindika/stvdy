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
| `Embedder` | `fastembed` `multilingual-e5-base`, 768 dims | 1 |
| `SearchProvider` | Tavily | 3 |
| `PaymentProvider` | `qris_gateway` | 13 |
| `PushProvider` | APNs (token auth) + FCM v1 | 12 |
| `Storage` | S3-compatible (MinIO in dev) | 1 |
| `Clock` | system clock; fixed clock in tests | 4 |

## Config

- `config/plans.toml` — plans, limits, credit costs, provider cost ceilings.
  Loaded and validated at startup; `plans.toml` is the authority for
  entitlements, and the `plans` table is a mirror for FK integrity and auditing.
- `config/billing_policy.toml` — per-platform purchase policy. `validate()`
  rejects a mobile entry that enables purchase UI, prices, or a CTA, so the file
  cannot be edited into an app-store policy violation.

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

### Phase 0 — keychain round trip (macOS): PASS

`src-tauri/src/keychain.rs` has a test that stores, reads, overwrites and
deletes a refresh token through the OS keychain. Result on macOS 27 (arm64):

```
test keychain::tests::keychain_round_trips_a_refresh_token ... ok
```

No keychain prompt appeared for the locally built binary. The shell also builds
and launches: `tauri build --debug --no-bundle` produces
`src-tauri/target/debug/stvdy-shell`, which runs and holds a window.

### Phase 1 — pending

Deep-link round trip (macOS + iOS simulator), then the mobile capability spike
(APNs/FCM token registration, iOS document intake and share sheet, Android
`content://` import). Results are appended here.

## Local cache

Native clients keep a SQLite cache plus an outbox, owned by the Rust shell
(`rusqlite`, `bundled`) — never by JS, so the sync engine and the database live
on the same side of the boundary. The web build has no local database and is
online-first. SQLCipher encryption is a config flag, off by default.
