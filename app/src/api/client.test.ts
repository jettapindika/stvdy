import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { API_BASE_URL, api, apiRaw, apiRequest, ensureFreshSession } from './client'
import { ApiError, apiErrorReason, networkReason } from './errors'
import { getAccessToken, saveSession, setAccessToken } from '../lib/tokens'
import { bearerOf, installFakePlatform, jsonResponse, mockFetch } from '../test/harness'
import type { FakeSecureStore } from '../test/harness'

/**
 * The API client's three load-bearing behaviours.
 *
 * These are not general HTTP tests. Each one covers a way this client can fail
 * silently in production: refreshing more than once (which kills the session
 * family, because rotation is single-use), mapping an error into the wrong shape
 * (which makes a screen branch on the wrong code), and keeping a session the
 * server has already refused.
 */

const REFRESH_TOKEN_KEY = 'stvdy.refresh_token'
const DEVICE_ID_KEY = 'stvdy.device_id'

let store: FakeSecureStore

beforeEach(() => {
  store = installFakePlatform()
  setAccessToken(null)
})

afterEach(() => {
  vi.unstubAllGlobals()
})

function authorizationOf(init: RequestInit): string | undefined {
  return (init.headers as Record<string, string> | undefined)?.authorization
}

describe('single-flight refresh', () => {
  it('refreshes once for concurrent 401s and retries every one of them', async () => {
    const calls = mockFetch((url, init) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(200, {
          access_token: 'access-2',
          refresh_token: 'refresh-2',
          expires_at: '2026-10-11T10:15:00Z',
        })
      }
      if (authorizationOf(init) === 'Bearer access-2') return jsonResponse(200, { at: url })
      return jsonResponse(401, {
        code: 'token_expired',
        message: 'access token expired',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })
    setAccessToken('access-1')

    const results = await Promise.all([
      api.get<{ at: string }>('/workspaces'),
      api.get<{ at: string }>('/devices'),
      api.get<{ at: string }>('/auth/me'),
    ])

    // The assertion this whole file exists for. Three concurrent 401s must
    // produce one rotation: the server treats a replayed refresh token as a
    // stolen one and revokes the entire family.
    expect(calls.filter((call) => call.url.endsWith('/auth/refresh'))).toHaveLength(1)

    // Every request still succeeded, on the new token.
    expect(results.map((result) => new URL(result.at).pathname)).toEqual([
      '/workspaces',
      '/devices',
      '/auth/me',
    ])
    expect(results[0]?.at).toBe(`${API_BASE_URL}/workspaces`)
    expect(calls.filter((call) => bearerOf(call) === 'Bearer access-2')).toHaveLength(3)

    // The rotated token replaced the spent one, so the next cold start works.
    expect(store.values.get(REFRESH_TOKEN_KEY)).toBe('refresh-2')
    expect(getAccessToken()).toBe('access-2')
  })

  it('retries once, not twice, when the retry is refused as well', async () => {
    let protectedCalls = 0
    const calls = mockFetch((url) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(200, {
          access_token: 'access-2',
          refresh_token: 'refresh-2',
          expires_at: '2026-10-11T10:15:00Z',
        })
      }
      protectedCalls += 1
      return jsonResponse(401, {
        code: 'unauthorized',
        message: 'device revoked',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })
    setAccessToken('access-1')

    await expect(api.get('/workspaces')).rejects.toMatchObject({ code: 'unauthorized' })

    // One original attempt, one retry. A loop here would hammer a server that
    // has already said no.
    expect(protectedCalls).toBe(2)
    expect(calls.filter((call) => call.url.endsWith('/auth/refresh'))).toHaveLength(1)
  })

  it('does not refresh when a public route refuses the credentials', async () => {
    const calls = mockFetch(() =>
      jsonResponse(401, {
        code: 'unauthorized',
        message: 'wrong password',
        min_plan: null,
        limit: null,
        used: null,
      }),
    )

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })

    await expect(
      apiRequest('/auth/login', {
        method: 'POST',
        body: { email: 'student@example.com', password: 'nope' },
        auth: false,
      }),
    ).rejects.toMatchObject({ code: 'unauthorized' })

    // A wrong password is an answer, not an expired session. Refreshing here
    // would sign the user out of a session they still hold elsewhere.
    expect(calls.some((call) => call.url.endsWith('/auth/refresh'))).toBe(false)
    expect(calls[0]?.init.headers).not.toHaveProperty('authorization')
  })
})

describe('the signed-out path', () => {
  it('clears the stored session when the server refuses the refresh', async () => {
    mockFetch((url) => {
      if (url.endsWith('/auth/refresh')) {
        return jsonResponse(401, {
          code: 'unauthorized',
          message: 'refresh token revoked',
          min_plan: null,
          limit: null,
          used: null,
        })
      }
      return jsonResponse(401, {
        code: 'token_expired',
        message: 'access token expired',
        min_plan: null,
        limit: null,
        used: null,
      })
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })
    setAccessToken('access-1')

    // The caller sees the failure it hit, not a second one invented by the
    // refresh: a screen needs to know which request failed.
    await expect(api.get('/workspaces')).rejects.toMatchObject({
      code: 'token_expired',
      status: 401,
    })

    // And the session is gone from both places, so nothing can restore it and
    // `useSession` can see that it ended.
    expect(store.values.has(REFRESH_TOKEN_KEY)).toBe(false)
    expect(store.values.has(DEVICE_ID_KEY)).toBe(false)
    expect(getAccessToken()).toBeNull()
  })

  it('reports "no session" instead of refreshing when nothing is stored', async () => {
    const calls = mockFetch(() => jsonResponse(200, {}))

    await expect(ensureFreshSession()).resolves.toBe(false)
    expect(calls).toHaveLength(0)
  })

  it('keeps the session when the server could not be reached at all', async () => {
    mockFetch(() => {
      throw new TypeError('Failed to fetch')
    })

    await saveSession({ refreshToken: 'refresh-1', deviceId: 'device-1' })
    setAccessToken('access-1')

    await expect(api.get('/workspaces')).rejects.toMatchObject({ code: 'network', status: 0 })

    // The sentence the user reads names the origin that was tried. Without it,
    // "the server could not be reached" is the same notice for a machine with no
    // network, an API that is not running, and an app pointed at an origin
    // nothing serves any more — three situations with three different fixes.
    const failure = await api.get('/workspaces').catch((error: unknown) => error)
    expect(apiErrorReason(failure as ApiError)).toBe(networkReason(API_BASE_URL))
    expect(networkReason(API_BASE_URL)).toContain(API_BASE_URL)

    // Offline is not the same as refused. Signing a user out because the network
    // dropped would cost them their session for a transient failure.
    expect(store.values.get(REFRESH_TOKEN_KEY)).toBe('refresh-1')
    expect(getAccessToken()).toBe('access-1')
  })
})

describe('error mapping', () => {
  it('maps the documented envelope onto ApiError', async () => {
    mockFetch(() =>
      jsonResponse(422, {
        code: 'limit_reached',
        message: 'workspaces limit reached',
        min_plan: 'pro',
        limit: 2,
        used: 2,
      }),
    )

    const error = await api.post('/workspaces', { name: 'Algorithms' }).catch((caught: unknown) => caught)

    expect(error).toBeInstanceOf(Error)
    expect(error).toMatchObject({
      name: 'ApiError',
      status: 422,
      code: 'limit_reached',
      minPlan: 'pro',
      limit: 2,
      used: 2,
    })
  })

  it('falls back to the status when the body is not the envelope', async () => {
    mockFetch(() => jsonResponse(502, 'upstream is down'))

    await expect(api.get('/workspaces')).rejects.toMatchObject({
      code: 'provider_error',
      status: 502,
      minPlan: null,
      limit: null,
      used: null,
    })
  })

  it('sends FormData untouched, with no content-type of its own', async () => {
    const calls = mockFetch(() => jsonResponse(201, { source: { id: 's1' } }))
    setAccessToken('access-1')

    const form = new FormData()
    form.append('kind', 'text')
    form.append('text', 'Hukum Ohm')
    await api.post('/workspaces/w1/sources', form)

    const headers = calls[0]?.init.headers as Record<string, string>
    // The browser generates the multipart boundary; setting the header by hand
    // omits it and the server cannot parse the body.
    expect(headers['content-type']).toBeUndefined()
    expect(calls[0]?.init.body).toBe(form)
    expect(headers.authorization).toBe('Bearer access-1')
  })

  it('resolves a 204 with no body', async () => {
    mockFetch(() => jsonResponse(204))
    setAccessToken('access-1')

    await expect(api.delete('/workspaces/w1')).resolves.toBeUndefined()
  })
})

/**
 * The document path, added for the exports and the cheatsheet preview.
 *
 * Those routes answer a document rather than JSON — `text/markdown`,
 * `application/pdf`, a whole `text/html` — and the request for one still has to
 * carry the bearer token and still has to go through the single-flight refresh. A
 * hand-rolled `fetch` at the call site would be a second place for the auth
 * contract to live, and the failure would be a download that 401s the first time
 * an access token expired.
 */
describe('requests whose body is a document', () => {
  it('returns the response untouched, with the bearer attached', async () => {
    const calls = mockFetch(() => ({
      ok: true,
      status: 200,
      text: async () => '# Jaringan',
    }) as Response)
    setAccessToken('access-1')

    const response = await apiRaw('/reports/r1/export?format=md')

    expect(await response.text()).toBe('# Jaringan')
    expect(calls[0]?.url).toBe(`${API_BASE_URL}/reports/r1/export?format=md`)
    expect(authorizationOf(calls[0]?.init ?? {})).toBe('Bearer access-1')
  })

  it('still turns a refusal into the typed error the screens branch on', async () => {
    mockFetch(() =>
      jsonResponse(403, {
        code: 'plan_required',
        message: 'pdf is not in this plan',
        min_plan: 'Pro',
        limit: null,
        used: null,
      }),
    )
    setAccessToken('access-1')

    // The document routes are plan-gated too, so a refusal has to arrive as the
    // same `ApiError` every other route produces: a screen that could only see a
    // status code would have to invent the copy for it.
    await expect(apiRaw('/reports/r1/export?format=pdf')).rejects.toMatchObject({
      code: 'plan_required',
      status: 403,
      minPlan: 'Pro',
    })
  })
})
