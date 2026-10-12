import { useQuery } from '@tanstack/react-query'

import { api } from '../client'
import { platform } from '@/platform'
import { hasLocalCache } from './synced'

/**
 * Events, as the Today timeline and the week's deadlines need them.
 *
 * The read is a **window**, and the window is required: `GET /events?from=&to=`
 * rejects a request without one, because a query with no start is a full-table
 * scan with no meaning. The window is also an **overlap** and not a containment —
 * a three-hour class that started before `from` and ends after it is on the
 * timeline the student is looking at, and a `starts_at between` filter drops it
 * with nothing on the page to say why. That rule is the server's, and this module
 * deliberately does not restate it: it sends the window and renders what comes
 * back.
 *
 * Four columns — `source`, `external_id`, `etag`, `connection_id` — are the
 * calendar sync's (A198), so nothing here writes them. `source` is read, because
 * a row that came from Google must not be presented as the student's own.
 */

export interface EventRow {
  id: string
  workspace_id: string | null
  kind: string
  title: string
  starts_at: string
  ends_at: string
  source: string
  task_id: string | null
  created_at?: string | null
  updated_at?: string
}

export const eventKeys = {
  window: (from: string, to: string) => ['events', from, to] as const,
}

/**
 * The events in a window, from the cache when there is one and from the route
 * otherwise.
 *
 * A cached read has no window to pass — the device holds what it was told about,
 * and filtering it here would be a second implementation of the overlap rule
 * above. So the cached path filters by *nothing* and lets the caller bucket: the
 * day buckets are `features/today/day.ts`'s, which is the one place local-day
 * logic lives (A205).
 */
export function useEvents(from: string, to: string) {
  return useQuery({
    queryKey: eventKeys.window(from, to),
    queryFn: async (): Promise<EventRow[]> => {
      if (hasLocalCache()) return await platform.sync.read<EventRow>('event')
      const body = await api.get<{ events: EventRow[] }>(
        `/events?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`,
      )
      return body.events
    },
  })
}

/**
 * Creates an event.
 *
 * The body carries the four fields a create needs and nothing the server owns:
 * `kind`, `title`, `starts_at` and `ends_at`. A deadline is an event with
 * `kind = 'deadline'` and `ends_at = starts_at` (A199) — there is no all-day flag
 * in the schema, and inventing one would give Phase 10 two ways to say the same
 * thing.
 */
export async function saveEvent(input: {
  id: string
  workspace_id: string | null
  kind: string
  title: string
  starts_at: string
  ends_at: string
  task_id: string | null
}): Promise<void> {
  if (hasLocalCache()) {
    await platform.sync.write('event', input.id, { ...input })
    return
  }
  await api.post<{ event: EventRow }>('/events', { ...input })
}
