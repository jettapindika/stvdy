import { api } from '../client'
import { platform } from '@/platform'

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
