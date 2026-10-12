import { useQuery } from '@tanstack/react-query'

import { api } from '../client'
import { platform } from '@/platform'
import { hasLocalCache } from './synced'

/**
 * Study sessions, and the two ways a device records one.
 *
 * A session is one of the six entities a client may write offline, and it is the
 * one with a **device** clock: the student's minutes are measured by the device
 * that was in the room, and the server accepts a `client_ts` for exactly that
 * reason (A204). The bound it enforces compares the session's length against the
 * client's *own* report of when it wrote the row, so a device whose clock runs
 * fast is not refused and a fabricated row still cannot claim more time than had
 * elapsed when it was written.
 *
 * The two paths are not two implementations of that rule — they are the two
 * transports the one rule arrives over, and the choice is made by the build:
 *
 * - a build with a local cache (the shell) writes through `platform.sync.write`,
 *   which is the outbox: the session is recorded now and pushed when there is a
 *   network, which is the whole point of an offline timer;
 * - the web build has no cache and no outbox (A14/A183), so `sync.write` refuses.
 *   It posts the same body to `POST /sessions`, which is the mutation's own shape
 *   — one validator serves both callers.
 *
 * What the two paths must **not** do is disagree about `client_ts`. On the REST
 * path the body carries it, because that route reads it from the body. On the
 * push path it must be absent: the mutation already carries it (the shell stamps
 * it when the write is queued) and `validate_session` refuses a payload whose
 * `client_ts` disagrees with the mutation's own — which it always would, by a
 * millisecond, since the two clocks are read at different moments. A payload that
 * named it would make every offline session a rejected mutation.
 */

/** A session as the device holds it, and as the timer produces one. */
export interface SessionRow {
  id: string
  workspace_id: string | null
  task_id: string | null
  started_at: string
  duration_min: number
  created_at?: string | null
  updated_at?: string
  /** Present only on a cached row: the pull ships tombstones. */
  deleted_at?: string | null
}

export const sessionKeys = {
  window: (from: string, to: string) => ['sessions', from, to] as const,
}

export const activityKeys = {
  window: (from: string, to: string) => ['activity', from, to] as const,
}

/** One session to record. `id` is minted when the timer *starts* (A206). */
export interface SessionInput {
  id: string
  startedAt: string
  durationMin: number
  taskId: string | null
  workspaceId: string | null
}

/**
 * Records one session.
 *
 * The id is the caller's, minted at the start of the timer and persisted with it,
 * so a flush after a restart is the *same* append-only id: the server's
 * `on conflict (id) do nothing` then makes a double flush a no-op, and a session
 * cannot be counted twice. That is why this function takes an id rather than
 * generating one — a fresh id per call would pass every other test here and
 * double-count the student's study minutes.
 */
export async function logSession(input: SessionInput): Promise<void> {
  const payload = {
    id: input.id,
    workspace_id: input.workspaceId,
    task_id: input.taskId,
    started_at: input.startedAt,
    duration_min: input.durationMin,
  }

  if (platform.runtime() === 'web') {
    await api.post<{ applied: boolean }>('/sessions', {
      ...payload,
      // The device's own report of when it wrote the row, which is the field the
      // bound reads. On the push path the mutation carries this instead.
      client_ts: new Date().toISOString(),
    })
    return
  }

  await platform.sync.write('study_session', input.id, payload)
}

/** The three instants a streak is made of, as `GET /activity` returns them. */
export interface ActivityResponse {
  reviews: string[]
  attempts: string[]
  sessions: string[]
}

/**
 * The sessions in a window, and the three instants a streak is made of.
 *
 * Both exist because the Today view needs them and neither is derivable from the
 * other: minutes need durations, and the streak needs the *days* on which a
 * review, an attempt or a session happened. `GET /activity` returns exactly those
 * three arrays and nothing else, in one round trip, because the *rule* that turns
 * them into a streak is the client's — the Today view has to work offline, and a
 * server route cannot (A205).
 *
 * The window is required by both routes (`from` is a 400 without it), and it is
 * the caller's: this module sends what it is given rather than guessing a horizon
 * the server would then have to agree with.
 */
export function useSessions(from: string, to: string) {
  return useQuery({
    queryKey: sessionKeys.window(from, to),
    queryFn: async (): Promise<SessionRow[]> => {
      if (hasLocalCache()) {
        const rows = await platform.sync.read<SessionRow>('study_session')
        return rows.filter((row) => row.deleted_at == null)
      }
      const body = await api.get<{ sessions: SessionRow[] }>(
        `/sessions?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`,
      )
      return body.sessions
    },
  })
}

/**
 * The three instants, from the cache when there is one and from `GET /activity`
 * otherwise.
 *
 * The cached path reads the three entities the streak is made of rather than
 * asking for a precomputed number, and that is deliberate: a server that returned
 * "5" would be a second implementation of the streak rule, and the two would
 * disagree the first time one was edited. What crosses the boundary is the
 * evidence; the count is derived in `day.ts`.
 */
export function useActivity(from: string, to: string) {
  return useQuery({
    queryKey: activityKeys.window(from, to),
    queryFn: async (): Promise<ActivityResponse> => {
      if (hasLocalCache()) {
        const [reviews, attempts, sessions] = await Promise.all([
          platform.sync.read<{ reviewed_at: string; deleted_at?: string | null }>(
            'flashcard_review',
          ),
          platform.sync.read<{ answered_at: string; deleted_at?: string | null }>('quiz_attempt'),
          platform.sync.read<{ started_at: string; deleted_at?: string | null }>('study_session'),
        ])
        const live = <T extends { deleted_at?: string | null }>(rows: T[]) =>
          rows.filter((row) => row.deleted_at == null)
        return {
          reviews: live(reviews).map((row) => row.reviewed_at),
          attempts: live(attempts).map((row) => row.answered_at),
          sessions: live(sessions).map((row) => row.started_at),
        }
      }
      return await api.get<ActivityResponse>(
        `/activity?from=${encodeURIComponent(from)}&to=${encodeURIComponent(to)}`,
      )
    },
  })
}
