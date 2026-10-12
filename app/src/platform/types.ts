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

export interface Platform {
  kind(): PlatformKind
  runtime(): Runtime
  secureStore: SecureStore
  notifications: NotificationsApi
  files: FilesApi
  deepLinks: DeepLinksApi
  openExternal(url: string): Promise<void>
  /** Haptics are a no-op off mobile. */
  haptic(kind: 'light' | 'medium' | 'success'): Promise<void>
  /** Desktop only; resolves false elsewhere. */
  checkForUpdate(): Promise<boolean>
  /** The local-first cache and outbox; the web build has neither (A14). */
  sync: SyncApi
  /**
   * The device-local key-value store. Available everywhere, including the web
   * build: unlike `sync`, this is not a claim about offline support, and the
   * focus timer's running state is the only thing that lives in it.
   */
  localState: LocalStateApi
}
