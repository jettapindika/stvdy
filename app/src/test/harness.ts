import { vi } from 'vitest'

import {
  installPlatform,
  type DeepLinkIntent,
  type NotificationTap,
  type PlatformKind,
  type PushApi,
  type PushToken,
  type Runtime,
  type SecureStore,
  type SyncApi,
  type SyncCredentials,
  type SyncReport,
  type UpdateProgress,
  type UpdateStatus,
} from '@/platform'
import { webPlatform } from '@/platform/web'

/**
 * Test doubles for the two things the API layer talks to: the platform's secure
 * store and `fetch`.
 *
 * The secure store is faked through `installPlatform`, which is the platform
 * adapter's own seam for exactly this (the Tauri shell installs its own adapter
 * the same way). Faking it here rather than mocking the module means the tests
 * exercise the real `tokens.ts` and the real client against a store that behaves
 * like a keychain, including the part that matters: what is in it afterwards.
 *
 * `fetch` is scripted per test rather than globally stubbed, because almost every
 * test here is about *which* requests were made and in what order, and that is
 * easier to read from a list of calls than from assertions spread across mocks.
 */

export interface FakeSecureStore {
  values: Map<string, string>
  deleted: string[]
}

/**
 * A platform whose deep links can be driven by a test.
 *
 * `current()` answers the intent the shell was launched with, and `emit` calls
 * every subscriber as a link arriving at a running app would. The app must
 * handle both, so a test that only exercises one of them proves half of the
 * contract.
 */
export interface FakeDeepLinks {
  emit(intent: DeepLinkIntent): void
  /** The intents `onOpen` handlers were registered for, for asserting teardown. */
  subscribers: number
}

/**
 * A sync surface a test can drive.
 *
 * The status is push-based in production (`Platform.sync.onStatus`), so the fake
 * has to be too: `set` decides what a fresh `status()` answers, and `emit` is what
 * the shell's own event would do — a report arriving while the control is mounted.
 * `writes` and `tombstones` are recorded rather than spied so a test can assert
 * what was queued and in which order.
 */
export interface FakeSync {
  /** The report a later `status()` answers with. */
  set(report: SyncReport): void
  /** Delivers a report to every `onStatus` subscriber, as the shell's event does. */
  emit(report: SyncReport): void
  writes: Array<{ entity: string; id: string; payload: Record<string, unknown> }>
  tombstones: Array<{ entity: string; id: string }>
  resets: number
  syncNows: number
  /** Every `configureSync` call, in order, for asserting the token handshake. */
  configured: SyncCredentials[]
  /** How many subscribers are live, for asserting an unsubscribe actually ran. */
  readonly subscribers: number
}

/** The report a fake platform answers before a test says otherwise. */
const DEFAULT_REPORT: SyncReport = {
  state: 'synced',
  code: null,
  lastSyncedAt: null,
  outbox: 0,
}

function fakeSync(overrides?: Partial<SyncApi>): { api: SyncApi; fake: FakeSync } {
  let report = DEFAULT_REPORT
  const handlers = new Set<(report: SyncReport) => void>()
  const fake: FakeSync = {
    set(next) {
      report = next
    },
    emit(next) {
      report = next
      for (const handler of handlers) handler(next)
    },
    writes: [],
    tombstones: [],
    resets: 0,
    syncNows: 0,
    configured: [],
    get subscribers() {
      return handlers.size
    },
  }

  const api: SyncApi = {
    async status() {
      return report
    },
    async read<T>(entity: string, id?: string): Promise<T[] | T | null> {
      // Both shapes answer the same way the shell does: an enumeration of a
      // device that has nothing cached is an **empty list**, and a single row it
      // never saw is `null`. Returning `null` for the list shape would be a
      // TypeError at the first `.filter()` in a consumer, which is a failure in
      // the test double rather than in the screen under test.
      void entity
      if (id !== undefined) return null
      const nothingCached: T[] = []
      return nothingCached
    },
    async write(entity, id, payload) {
      fake.writes.push({ entity, id, payload })
    },
    async tombstone(entity, id) {
      fake.tombstones.push({ entity, id })
    },
    async syncNow() {
      fake.syncNows += 1
      return report
    },
    async reset() {
      fake.resets += 1
      report = DEFAULT_REPORT
    },
    async onStatus(handler) {
      handlers.add(handler)
      return () => {
        handlers.delete(handler)
      }
    },
    ...overrides,
  }

  return { api, fake }
}

/**
 * A push capability a test can drive, installed the way a phone has one.
 *
 * Push is the one capability `Platform` declares as optional, so the fake has to be
 * able to produce *both* states: `installFakePlatform` leaves `push` absent unless a
 * test asks for it, which is what makes the absent-capability path assertable rather
 * than assumed. When it is installed, `emitToken` is what the plugin's rotation event
 * would do and `emitNotification` is a tap.
 */
export interface FakePush {
  /** The token `token()` answers with, and how many times it was asked. */
  token(): PushToken
  readonly asks: number
  /** Delivers a rotation to every `onToken` subscriber, as the plugin's event does. */
  emitToken(token: string): void
  /** Delivers a tap to every `onNotification` subscriber. */
  emitNotification(tap: NotificationTap): void
  /** How many subscribers are live, for asserting an unsubscribe actually ran. */
  readonly tokenSubscribers: number
  readonly notificationSubscribers: number
}

function fakePush(initial: PushToken): { api: PushApi; fake: FakePush } {
  let current = initial
  let asks = 0
  const tokenHandlers = new Set<(token: string) => void>()
  const notificationHandlers = new Set<(tap: NotificationTap) => void>()

  const fake: FakePush = {
    token: () => current,
    get asks() {
      return asks
    },
    emitToken(token) {
      current = { ...current, token }
      for (const handler of tokenHandlers) handler(token)
    },
    emitNotification(tap) {
      for (const handler of notificationHandlers) handler(tap)
    },
    get tokenSubscribers() {
      return tokenHandlers.size
    },
    get notificationSubscribers() {
      return notificationHandlers.size
    },
  }

  const api: PushApi = {
    async token() {
      asks += 1
      return current
    },
    async onToken(handler) {
      tokenHandlers.add(handler)
      return () => {
        tokenHandlers.delete(handler)
      }
    },
    async onNotification(handler) {
      notificationHandlers.add(handler)
      return () => {
        notificationHandlers.delete(handler)
      }
    },
  }

  return { api, fake }
}

export function installFakePlatform(
  kind: PlatformKind = 'web',
  options: {
    current?: DeepLinkIntent | null
    deepLinks?: boolean
    sync?: Partial<SyncApi>
    /**
     * Overridden so a test can drive the Updates panel. The panel's whole subject
     * is what it does with a manifest's answer, so the fake has to be able to
     * return each of `UpdateStatus`'s four cases rather than always
     * `unsupported`.
     */
    update?: {
      checkForUpdate: () => Promise<UpdateStatus>
      installUpdate: (onProgress: (progress: UpdateProgress) => void) => Promise<void>
    }
    /** Overridden so a test can render the native-only paths (A14). */
    runtime?: Runtime
    /**
     * Installs a push capability, which is what a phone has and a browser does not.
     *
     * Absent by default, and that default is the assertion: `Platform.push` is
     * optional precisely so the web build and desktop can be without it, and a fake
     * that always installed one would make every test exercise a platform that does
     * not exist while the absent path — the one that ships to every non-phone —
     * went untested.
     */
    push?: PushToken
  } = {},
): FakeSecureStore & {
  deepLinks: FakeDeepLinks
  opened: string[]
  sync: FakeSync
  push: FakePush | null
} {
  const values = new Map<string, string>()
  const deleted: string[] = []
  // Every URL the app handed to the system browser, in order. Recorded rather
  // than spied so a test can assert *what* was opened and that nothing else was:
  // a web citation that navigated the app would show up here as a missing entry
  // and in the DOM as a page that is no longer the answer.
  const opened: string[] = []

  const secureStore: SecureStore = {
    async set(key, value) {
      values.set(key, value)
    },
    async get(key) {
      return values.get(key) ?? null
    },
    async delete(key) {
      deleted.push(key)
      values.delete(key)
    },
  }

  const handlers = new Set<(intent: DeepLinkIntent) => void>()
  const deepLinks: FakeDeepLinks = {
    emit(intent) {
      // The shell's handler is typed to its own union; the fake's `emit` takes
      // the wider pinned union so a test can deliver `auth_ticket` before that
      // member lands in `platform/types.ts`.
      for (const handler of handlers) handler(intent as DeepLinkIntent)
    },
    get subscribers() {
      return handlers.size
    },
  }

  const { api: syncApi, fake: sync } = fakeSync(options.sync)
  const push = options.push === undefined ? null : fakePush(options.push)

  installPlatform(
    {
      ...webPlatform(kind),
      // A native kind has a runtime with a local database; `web` does not. The
      // fake follows the kind rather than always claiming `web`, because the
      // online-only sync path is chosen by `runtime()` and a fake that lied here
      // would make every native test exercise the web branch (A14).
      runtime: () => options.runtime ?? (kind === 'web' ? 'web' : 'tauri'),
      secureStore,
      sync: syncApi,
      ...(options.update ?? {}),
      ...(push === null ? {} : { push: push.api }),
      async openExternal(url) {
        opened.push(url)
      },
      ...(options.deepLinks
        ? {
            deepLinks: {
              async current() {
                return (options.current ?? null) as DeepLinkIntent | null
              },
              async onOpen(handler: (intent: DeepLinkIntent) => void) {
                handlers.add(handler)
                return () => {
                  handlers.delete(handler)
                }
              },
            },
          }
        : {}),
    },
    {
      // Recorded rather than spied, so a test asserts what the engine was handed:
      // the token handshake is the one thing `Platform.sync`'s seven members cannot
      // carry (A186).
      async configureSync(credentials) {
        sync.configured.push(credentials)
      },
    },
  )
  return { values, deleted, deepLinks, opened, sync, push: push?.fake ?? null }
}

/**
 * A platform with no usable keychain, which is what the web adapter really is:
 * `set` and `delete` throw, and `get` answers null. The app has to keep working
 * in the tab and simply be unable to restore the session later.
 */
export function installFailingSecureStore(kind: PlatformKind = 'web'): void {
  installPlatform(
    {
      ...webPlatform(kind),
    secureStore: {
      async set() {
        throw new Error('secure storage is not available in the web build')
      },
      async get() {
        return null
      },
      async delete() {
        throw new Error('secure storage is not available in the web build')
      },
    },
  })
}

/**
 * A response shaped like the ones `fetch` returns, built from a status and a JSON
 * body. Only the three members the client reads are implemented, so a test never
 * depends on a global `Response` that jsdom does not provide.
 */
export function jsonResponse(status: number, body?: unknown): Response {
  return {
    ok: status >= 200 && status < 300,
    status,
    text: async () => (body === undefined ? '' : JSON.stringify(body)),
  } as Response
}

/**
 * A response whose body arrives in the chunks given, for the SSE reader.
 *
 * The chunk list is the test's, not the platform's: a reader hands back whatever
 * byte boundaries it happens to produce, and the boundary that matters is the one
 * in the middle of a frame. Here the test chooses it.
 *
 * Only `ok`, `status` and `body` are implemented, for the same reason
 * `jsonResponse` implements three members: the code under test must not depend on
 * a global `Response` that the environment may not provide.
 */
export function streamResponse(chunks: string[], status = 200): Response {
  const encoder = new TextEncoder()
  return {
    ok: status >= 200 && status < 300,
    status,
    body: new ReadableStream<Uint8Array>({
      start(controller) {
        for (const chunk of chunks) controller.enqueue(encoder.encode(chunk))
        controller.close()
      },
    }),
    text: async () => chunks.join(''),
  } as unknown as Response
}

/**
 * A response the test pushes into, one frame at a time, and can fail on demand.
 *
 * Needed wherever the assertion is about *when* a chunk arrives rather than about
 * what the frames mean. A body that is enqueued all at once is read in a single
 * microtask chain, so React batches every token into one render and the
 * intermediate states a streaming screen exists to show are never observable.
 * Here the test decides the moment.
 *
 * `fail` exists because erroring a `ReadableStream` discards whatever is still
 * queued, so a body that enqueues a token and then errors never delivers the
 * token: a dropped connection has to be delivered *after* the chunks, not with
 * them.
 */
export interface ManualStream {
  response: Response
  push(chunk: string): void
  close(): void
  fail(error: Error): void
  /** Fails the body when this request's signal aborts, as a real fetch does. */
  attach(signal: AbortSignal | null | undefined): void
}

export function manualStreamResponse(status = 200): ManualStream {
  const encoder = new TextEncoder()
  let controller: ReadableStreamDefaultController<Uint8Array> | null = null
  let finished = false

  const response = {
    ok: status >= 200 && status < 300,
    status,
    body: new ReadableStream<Uint8Array>({
      start(inner) {
        controller = inner
      },
    }),
    text: async () => '',
  } as unknown as Response

  function terminal(action: (inner: ReadableStreamDefaultController<Uint8Array>) => void) {
    if (controller === null || finished) return
    finished = true
    action(controller)
  }

  return {
    response,
    push(chunk) {
      if (controller === null || finished) return
      controller.enqueue(encoder.encode(chunk))
    },
    close() {
      terminal((inner) => {
        inner.close()
      })
    },
    fail(error) {
      terminal((inner) => {
        inner.error(error)
      })
    },
    attach(signal) {
      signal?.addEventListener('abort', () => {
        // The name is what the reader checks: a cancellation is the caller's own
        // action, not a failure to report.
        const aborted = new Error('the request was aborted')
        aborted.name = 'AbortError'
        this.fail(aborted)
      })
    },
  }
}

export interface RecordedCall {
  url: string
  init: RequestInit
}

export function mockFetch(
  handler: (url: string, init: RequestInit) => Response | Promise<Response>,
): RecordedCall[] {
  const calls: RecordedCall[] = []
  vi.stubGlobal(
    'fetch',
    vi.fn(async (input: RequestInfo | URL, init?: RequestInit) => {
      const url = typeof input === 'string' ? input : input.toString()
      const request = init ?? {}
      calls.push({ url, init: request })
      return handler(url, request)
    }),
  )
  return calls
}

/** The `authorization` header a request carried, if any. */
export function bearerOf(call: RecordedCall): string | null {
  const headers = call.init.headers as Record<string, string> | undefined
  return headers?.authorization ?? null
}

/**
 * jsdom has no `matchMedia`, and the shell's form-factor hook subscribes to it.
 * Without this the whole app throws on render rather than reporting a layout.
 */
export function stubViewport(width: number): void {
  Object.defineProperty(window, 'innerWidth', { writable: true, configurable: true, value: width })
  vi.stubGlobal('matchMedia', (query: string) => {
    const min = Number(/min-width:\s*(\d+)px/.exec(query)?.[1] ?? 0)
    return {
      matches: width >= min,
      media: query,
      addEventListener: () => {},
      removeEventListener: () => {},
      addListener: () => {},
      removeListener: () => {},
      onchange: null,
      dispatchEvent: () => false,
    } as unknown as MediaQueryList
  })
}

/**
 * Every number anywhere in a response body, so a test's allowed set is the
 * payload itself.
 *
 * This is how the plan screens assert the honesty rule rather than asserting
 * that the numbers appear: a screen that hardcodes a number passes "the numbers
 * are shown" and fails this, because the number is not in the fixture. Strings
 * are skipped deliberately — a payload's dates are ISO instants the screen
 * formats rather than prints, and printing one raw is a different defect with
 * its own assertion.
 */
export function fixtureNumbers(value: unknown): number[] {
  if (typeof value === 'number') return [value]
  if (typeof value === 'string' || value === null || typeof value !== 'object') return []
  return Object.values(value).flatMap(fixtureNumbers)
}
