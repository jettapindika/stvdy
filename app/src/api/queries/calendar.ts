import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'

/**
 * The calendar's routes: the feed token, the ICS import, the connection list and
 * the manual sync.
 *
 * Two rules this module exists to keep, both of them honesty rather than
 * convenience:
 *
 * - **No token of any kind is ever in this file's state except the ICS feed
 *   token.** `GET /calendar/connections` answers `ConnectionView` and never a
 *   refresh token (A249), and the feed token is the one secret the settings screen
 *   is required to *show* (A242) — it is a capability URL the student copies into
 *   their calendar client, so it is read, displayed and rotated here and nowhere
 *   else. Nothing here is persisted to storage: the token lives in the query cache
 *   for as long as the panel is mounted.
 * - **The Google consent URL is opened by the shell, never by this app.** The
 *   route answers `{ url }` and the caller hands it to `platform.openExternal`,
 *   i.e. the system browser. An embedded webview would be a page this app draws
 *   around someone else's login form, which the spec forbids by name.
 *
 * The two halves of the matrix are deliberately different here, and getting them
 * backwards is the easy mistake: **the ICS feed and the ICS import exist on every
 * plan** — `config/plans.toml` gives free the `ICS feed + import` row — while the
 * **Google connection is pro-only** (`google_calendar = false` on free,
 * `google_accounts_max = 0`). So nothing in this file gates the feed or the ICS
 * field; the only gate is around "Connect Google Calendar", and it is a
 * `LockedFeature` at the panel.
 */

/**
 * One connection, as `GET /calendar/connections` reports it (A249).
 *
 * `error` is the connection's own stored reason — `recurrence_truncated` for an
 * expansion that hit the bound, a parser's complaint for a feed that half-read
 * (A247) — and it is rendered beside the connection rather than swallowed. That is
 * gate 7: the panel must not claim a connection is healthy while it holds an
 * error.
 */
export interface Connection {
  id: string
  /** `google` | `ics`. */
  provider: string
  /** The Google account's address, or null. The one Google fact worth storing. */
  account_email: string | null
  status: string
  error: string | null
  last_synced_at: string | null
  study_calendar_id: string | null
}

export const calendarKeys = {
  connections: ['calendar', 'connections'] as const,
  feedToken: ['calendar', 'ics-token'] as const,
}

/** Every connection on the account, from `GET /calendar/connections`. */
export function useConnections() {
  return useQuery({
    queryKey: calendarKeys.connections,
    queryFn: async (): Promise<Connection[]> => {
      const body = await api.get<{ connections: Connection[] }>('/calendar/connections')
      return body.connections
    },
  })
}

/**
 * The caller's ICS feed token, or `null` when none has been generated.
 *
 * `null` is a real answer and is rendered as one: "no feed yet, press Regenerate
 * to make one". The alternative — minting a token on read — would rotate a
 * student's feed every time they opened settings, which breaks every client they
 * had already subscribed.
 */
export function useFeedToken() {
  return useQuery({
    queryKey: calendarKeys.feedToken,
    queryFn: async (): Promise<string | null> => {
      const body = await api.get<{ ics_feed_token: string | null }>('/calendar/ics-token')
      return body.ics_feed_token
    },
  })
}

/** Rotates the feed token: revoke-and-replace in one call (A242). */
export function useRegenerateFeedToken() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () =>
      api.post<{ ics_feed_token: string }>('/calendar/ics-token/regenerate', {}),
    onSuccess: (body) => {
      queryClient.setQueryData(calendarKeys.feedToken, body.ics_feed_token)
    },
  })
}

/**
 * `POST /calendar/ics`. Validates the URL, writes the connection and queues the
 * first sync, which answers 201 with the connection — never with the imported
 * events, because the import is a job.
 */
export function useAddIcs() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (url: string) =>
      api.post<{ connection: Connection }>('/calendar/ics', { url }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: calendarKeys.connections }),
  })
}

/**
 * `DELETE /calendar/connections/:id`.
 *
 * Deleting the connection tombstones the events it imported, in the same
 * transaction on the server (the rows are read-only to a client, so nothing else
 * could remove them). The event window is invalidated too, or the grid would keep
 * showing a timetable the student just disconnected.
 */
export function useRemoveConnection() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (connectionId: string) => api.delete(`/calendar/connections/${connectionId}`),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: calendarKeys.connections })
      await queryClient.invalidateQueries({ queryKey: ['events'] })
    },
  })
}

/**
 * `GET /calendar/google/connect` → `{ url }`.
 *
 * The route refuses a free account with 403 `plan_required` through the
 * entitlement engine (A245), which is the *server's* enforcement; the panel also
 * shows a `LockedFeature` instead of the button, so a free user is told why rather
 * than being offered a button that fails.
 */
export async function googleConnectUrl(): Promise<string> {
  const body = await api.get<{ url: string }>('/calendar/google/connect')
  return body.url
}

/**
 * `POST /calendar/sync` → 202 `{ queued }`.
 *
 * `queued` counts the jobs actually inserted, so a second press inside the same
 * minute answers `0` — the per-minute dedupe key (A238) bounds a retry loop
 * without a second rate limiter, and the panel says "queued" rather than claiming
 * a sync finished. The pass runs in the worker; this route does not wait for it.
 */
export function useManualSync() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: () => api.post<{ queued: number }>('/calendar/sync', {}),
    onSuccess: async () => {
      await queryClient.invalidateQueries({ queryKey: calendarKeys.connections })
    },
  })
}

/**
 * The feed URL a student pastes into their calendar client.
 *
 * Built here rather than returned by the server, because the URL's host is the
 * API origin this build talks to and a server-composed absolute URL would be
 * wrong behind a proxy. The path shape is the documented one
 * (`GET /cal/<token>.ics`); the router registers `/cal/{token}` and requires the
 * `.ics` suffix, so a URL without it is a 404 and not a second address for the
 * same resource.
 */
export function feedUrl(origin: string, token: string): string {
  return `${origin.replace(/\/+$/, '')}/cal/${encodeURIComponent(token)}.ics`
}
