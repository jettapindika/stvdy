export type PlatformKind = 'web' | 'macos' | 'windows' | 'ios' | 'android'
export type Runtime = 'web' | 'tauri'

/**
 * What sync is doing, as the shell reports it.
 *
 * The four states the control *displays* are the spec's: `synced`, `syncing`,
 * `offline` and `needs_attention`. There is deliberately no generic `error`:
 * a state the user can act on is one of these four, and "error" would be a word
 * that names neither the cause nor the next step (R-27).
 *
 * `revoked` is the engine's fifth, terminal state (`crates/sync`'s `SyncState`),
 * and it is in the union because it is a value `sync_status` can return: this
 * device's session is gone, its local data has been wiped, and the app signs out
 * (A186). It is never rendered as a status word — there is no status to show once
 * the account is leaving — but a value the shell can send has to be
 * representable here rather than silently mistyped as one of the four.
 */
export type SyncStatus = 'synced' | 'syncing' | 'offline' | 'needs_attention' | 'revoked'

/**
 * One device's sync state, from stored facts only (A187).
 *
 * `lastSyncedAt` is the **stored** timestamp of the last successful pass, never a
 * clock read at render time: a screen that invents "Last synced just now" is
 * claiming a fact it does not have (roadmap gate 7a). `outbox` is how many local
 * writes the server has not accepted yet, and it is what stops the control from
 * ever reading "Synced" over a pending write (gate 7b).
 */
export interface SyncReport {
  state: SyncStatus
  /** The server's own rejection code, present only for `needs_attention` (gate 7d). */
  code: string | null
  /** The stored time of the last successful pass, or null when there is none. */
  lastSyncedAt: string | null
  /** Local writes queued but not yet accepted by the server. */
  outbox: number
}

/**
 * The local-first half of sync, over the shell's SQLite cache (A186).
 *
 * The web build has no local database and no outbox (A14), so its implementation
 * refuses `read`/`write`/`tombstone` loudly rather than accepting a write it could
 * never deliver. That refusal is the point: a build that queues a mutation it has
 * nowhere to store is worse than one that says so.
 */
export interface SyncApi {
  /** The status, derived from stored facts only (A187). */
  status(): Promise<SyncReport>
  /**
   * Every cached row of an entity, newest first.
   *
   * This is the same member as the single-row read below and the same shell
   * command (`sync_read` with no `id`): a device that is offline still has to
   * enumerate today's tasks, and a second accessor for the same call would be the
   * second shape A186 exists to prevent.
   */
  read<T>(entity: string): Promise<T[]>
  /** One cached row, or null when this device never saw it. */
  read<T>(entity: string, id: string): Promise<T | null>
  /** Writes a row locally and queues the mutation for the next pass. */
  write(entity: string, id: string, payload: Record<string, unknown>): Promise<void>
  /** Queues a delete for the next pass. */
  tombstone(entity: string, id: string): Promise<void>
  /** Runs one pass now. */
  syncNow(): Promise<SyncReport>
  /** Wipes the local cache **and** the outbox, then resyncs from zero (A186). */
  reset(): Promise<void>
  /** Subscribes to status changes; resolves to an unsubscribe function. */
  onStatus(handler: (report: SyncReport) => void): Promise<() => void>
}

export interface SecureStore {
  /** Stores a secret in the OS keychain/keystore. Web throws. */
  set(key: string, value: string): Promise<void>
  get(key: string): Promise<string | null>
  delete(key: string): Promise<void>
}

export interface NotificationsApi {
  /** Requests permission and returns whether notifications may be shown. */
  requestPermission(): Promise<boolean>
  notify(title: string, body: string): Promise<void>
  /**
   * Schedules a local notification. Mobile honours `at` and fires while the app
   * is closed; desktop and web cannot schedule, so they deliver immediately. * callers must treat scheduling as best-effort and not assume parity.
   */
  schedule(title: string, body: string, at: Date): Promise<void>
}

/**
 * This install's push token, as the shell reports it (A329).
 *
 * `provider` is derived by the **shell** from the platform — `"apns"` on iOS, `"fcm"`
 * on Android — and not read from the token, because the server's `PUSH_PROVIDERS` is
 * a closed set and a client that guessed it wrong would write a row that never
 * delivers. It is a plain `string` here rather than a union for the same reason: the
 * vocabulary is `push::provider_for`'s, and a second union in this file would be a
 * second place for the two to disagree.
 */
export interface PushToken {
  /** `"apns"` or `"fcm"`, as `push::provider_for` decided. */
  provider: string
  token: string
}

/**
 * A notification the student tapped, as the plugin delivers it.
 *
 * `data` carries the push payload's data keys, which for a reminder are the
 * `reminder_id` and `kind` the server sent. It is optional because the plugin emits
 * the same click event for a **local** notification — one of
 * `NotificationsApi.schedule`'s — and that has no payload at all, so the router has
 * to handle a tap that says nothing about where it came from.
 */
export interface NotificationTap {
  /** The plugin's own notification id, or `-1` when it had none. */
  id: number
  data?: Record<string, string> | null
}

/**
 * Push registration, which only a phone has (A329).
 *
 * **Optional on `Platform`, and the only member that is.** Every other capability is
 * declared on every adapter and degrades, because in each case there is a *something*
 * the caller can do on that platform: the web build's `secureStore.set` throws and
 * its `get` answers null, `sync.write` refuses by naming the REST route that works
 * instead, `deepLinks.current` answers null, `checkForUpdate` answers `unsupported`.
 * There is no such answer for push. A browser tab and a laptop have no APNs or FCM
 * token to register and no notification of theirs to be tapped, so the honest shape
 * is that the capability is not here, and a caller that reaches for it has to say
 * what it does instead. `undefined` is that statement; a stub that resolved would be
 * a registration that silently never happened.
 *
 * This is the shape `PlatformCapabilities.configureSync` already uses for the one
 * other capability that only the native adapters have, and it is why the mobile
 * adapter is the only one that carries this member.
 */
export interface PushApi {
  /**
   * This device's token, from the shell's `push_token` command.
   *
   * Rejects where the platform has no push provider. That cannot be reached through
   * this member — it only exists where the shell compiled a provider — but the
   * command keeps one shape on every platform, and its desktop answer is an error
   * rather than a fabricated `"apns"` token for a Mac.
   */
  token(): Promise<PushToken>
  /**
   * Subscribes to the token the OS re-issued; resolves to an unsubscribe (A329).
   *
   * The callback carries the token string from the plugin's own `push-token` event,
   * which is the only thing that hears a rotation. It deliberately does not carry a
   * provider: that mapping is the shell's, and the caller re-reads the pair through
   * `token()` — one mapping, one PUT.
   */
  onToken(handler: (token: string) => void): Promise<() => void>
  /**
   * Subscribes to a tap on a notification; resolves to an unsubscribe.
   *
   * A tap is the only surface a push is guaranteed to produce (A316): a silent push
   * may be dropped, throttled or never delivered at all, but a tap is a launch or a
   * foreground transition the app will see. Subscribing also asks the plugin to
   * *replay* a tap that arrived before anything was listening — which is what makes
   * a cold start route, and is the notification half of the shell's
   * `deep_link_current`.
   */
  onNotification(handler: (tap: NotificationTap) => void): Promise<() => void>
}

export interface FilesApi {
  /** Opens a picker and returns local paths, or null when cancelled. */
  pick(options?: { multiple?: boolean; mimeTypes?: string[] }): Promise<string[] | null>
  /** Reads a picked file as bytes. */
  read(path: string): Promise<Uint8Array>
}

/**
 * A classified incoming URL, as the shell delivers it. The SPA never sees a raw
 * URL string: `auth_callback` carries the OAuth `code`/`state` to exchange,
 * `calendar` carries a calendar-return's query parameters, and `ignored` says a
 * URL was not actionable (with a coarse reason safe to log).
 */
export type DeepLinkIntent =
  | { action: 'auth_callback'; payload: { code: string; state: string } }
  // The Google login return: an opaque single-use ticket, never tokens, because
  // a deep link lands in browser history and the OS URL handler (A48).
  | { action: 'auth_ticket'; payload: { ticket: string } }
  | { action: 'calendar'; payload: { params: Record<string, string> } }
  | { action: 'ignored'; payload: { reason: string } }

export interface DeepLinksApi {
  /** The classified URL the app was launched with, if any. */
  current(): Promise<DeepLinkIntent | null>
  /** Subscribes to incoming links; resolves to an unsubscribe function. */
  onOpen(handler: (intent: DeepLinkIntent) => void): Promise<() => void>
}

/**
 * The device-local key-value store, for the one fact that is about *this device*
 * rather than about the student (A206).
 *
 * Two things this is deliberately not. It is not the cache: `sync` owns the rows
 * that came from the server, and a value here is never pushed, never pulled and
 * never counted. And it is not for anything the student would expect to find on
 * another device — the focus timer's "I am running" marker is the whole
 * client-side use, because an unfinished session is not a fact about their study.
 *
 * `get` answers `null` for a key that was never written, for one that was
 * deleted, and for a stored value this build cannot read: every one of those means
 * "this device has nothing to resume", and a throw would turn the timer's mount
 * into a crash rather than an empty state.
 */
export interface LocalStateApi {
  /** Reads a JSON value by key, or null. Never throws for a missing key. */
  get<T>(key: string): Promise<T | null>
  set(key: string, value: unknown): Promise<void>
  delete(key: string): Promise<void>
}

/**
 * What a check for updates found, as the shell reports it (A336).
 *
 * Four cases, and each is a different thing the user can do next:
 *
 * - `unsupported` — this build has no update channel. The web build has no
 *   installer, and mobile ships through TestFlight and Play, where the Tauri
 *   updater declares `ios: none, android: none` (A335). This is deliberately a
 *   *value* rather than a rejection: a screen that has to catch an exception to
 *   learn the platform has no updater ends up rendering a disabled button, and a
 *   disabled "Check for updates" on a phone is a promise the platform cannot keep.
 * - `up-to-date` — the check ran and the manifest announced nothing newer. It
 *   carries `current` because the version the user is running is a fact the check
 *   established, not a value to be guessed at render time.
 * - `available` — the manifest announced a newer version. `version` and `notes`
 *   are **verbatim from the manifest** and are the only two facts about the
 *   release this app has: it never composes a "what's new" line, never computes a
 *   download size, and never labels a release a "security update" it was not told
 *   about (roadmap gate 7a, A336).
 * - `error` — the check did not complete. This is a state of its own precisely so
 *   a failed check cannot read as "you are up to date": a user who cannot detect
 *   that failure is a user who believes they are patched when they are not.
 */
export type UpdateStatus =
  | { kind: 'unsupported' }
  | { kind: 'up-to-date'; current: string }
  | { kind: 'available'; current: string; version: string; notes: string | null }
  | { kind: 'error'; message: string }

/**
 * How far an install has got, as a **phase and never a number**.
 *
 * The updater's download events carry byte counts and sometimes a total, and it
 * would be easy to render "43%". This union deliberately has nowhere to put one:
 * `contentLength` is optional in the plugin's own `DownloadEvent`, so a percentage
 * is computable on some downloads and not others, and a progress bar that stalls
 * at 100% or jumps backwards is a claim about data the app does not have (A336,
 * roadmap gate 7a). A phase is a fact the app always has.
 */
export type UpdateProgress =
  | { phase: 'downloading' }
  | { phase: 'installing' }
  | { phase: 'restart-required' }

export interface Platform {
  kind(): PlatformKind
  runtime(): Runtime
  secureStore: SecureStore
  notifications: NotificationsApi
  /**
   * Push registration. **Absent on the web build and on desktop**, which have no
   * push provider: see `PushApi` for why this member is the one that is optional
   * rather than degrading.
   */
  push?: PushApi
  files: FilesApi
  deepLinks: DeepLinksApi
  openExternal(url: string): Promise<void>
  /** Haptics are a no-op off mobile. */
  haptic(kind: 'light' | 'medium' | 'success'): Promise<void>
  /**
   * Asks the update endpoint what it has, and reports it (A336).
   *
   * Never throws: a failed check is `{ kind: 'error' }`, because the one bug this
   * call can have that a user cannot see is a failure that renders as success.
   */
  checkForUpdate(): Promise<UpdateStatus>
  /**
   * Downloads and installs the update the last check found, reporting phases.
   *
   * Rejects when there is nothing to install (no check has run, or it found
   * nothing), rather than re-checking: re-checking here could install a different
   * version from the one the panel showed the user.
   */
  installUpdate(onProgress: (progress: UpdateProgress) => void): Promise<void>
  /** The local-first cache and outbox; the web build has neither (A14). */
  sync: SyncApi
  /**
   * The device-local key-value store. Available everywhere, including the web
   * build: unlike `sync`, this is not a claim about offline support, and the
   * focus timer's running state is the only thing that lives in it.
   */
  localState: LocalStateApi
}
