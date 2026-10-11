import { vi } from 'vitest'

import { installPlatform, type DeepLinkIntent, type PlatformKind, type SecureStore } from '@/platform'
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

export function installFakePlatform(
  kind: PlatformKind = 'web',
  options: { current?: DeepLinkIntent | null; deepLinks?: boolean } = {},
): FakeSecureStore & { deepLinks: FakeDeepLinks } {
  const values = new Map<string, string>()
  const deleted: string[] = []

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

  installPlatform({
    ...webPlatform(kind),
    secureStore,
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
  })
  return { values, deleted, deepLinks }
}

/**
 * A platform with no usable keychain, which is what the web adapter really is:
 * `set` and `delete` throw, and `get` answers null. The app has to keep working
 * in the tab and simply be unable to restore the session later.
 */
export function installFailingSecureStore(kind: PlatformKind = 'web'): void {
  installPlatform({
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
