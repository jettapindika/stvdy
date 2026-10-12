import { useQuery } from '@tanstack/react-query'

import { api } from '../client'

/**
 * The reminder feed, and the one write a client makes about a reminder.
 *
 * **Registration is the client's, and it is the same rule as the server's.** The
 * spec says clients may schedule local notifications from synced events and tasks
 * so reminders work offline; `POST /reminders` is what tells the server the same
 * reminder exists, and it is idempotent by the dedupe key (A284) — two devices
 * computing 19:00 and 19:01 produce the same key, so the second write answers
 * `applied: false` and the row keeps the **first** `fire_at`. That is why the
 * response carries the flag: a client that retries after a lost response can tell
 * whether its write landed without re-reading.
 *
 * **`GET /reminders` is per device.** An ack records a
 * `reminder_deliveries` row keyed by `(reminder_id, 'web', device_id)`, so the
 * same reminder still shows on the student's other phone. A per-account ack would
 * silence a reminder on a device the student had not looked at yet.
 *
 * Neither call is in the offline-write list (A283): a registration requires
 * connectivity and queues nothing. The local schedule is what covers the offline
 * case, and it needs no server at all.
 */

/** A reminder as the feed reports it. The row's own columns and nothing derived. */
export interface ReminderRow {
  id: string
  /** `task_due`, `event_soon` or `cards_due`. */
  kind: string
  /** The task's or event's id. Null for the day-granular card reminder. */
  subject_id: string | null
  /** The reminder's local date in the student's zone, `YYYY-MM-DD`. */
  local_date: string
  fire_at: string
  status: string
  /** Why it will not be delivered, when it will not: `quiet_hours`, `already_past`, … */
  skip_reason: string | null
}

export const reminderKeys = {
  feed: ['reminders', 'feed'] as const,
}

/**
 * The in-app channel's feed: due, pending, and not yet acknowledged by this
 * device.
 *
 * A `skipped` reminder stays in the feed on purpose (A286): a reminder that
 * silently never arrives is what the student reports as "reminders are broken",
 * and the reason is the only thing that separates "quiet hours moved it past its
 * subject" from "the push token is dead".
 */
export function useReminders(enabled = true) {
  return useQuery({
    queryKey: reminderKeys.feed,
    queryFn: async (): Promise<ReminderRow[]> => {
      const body = await api.get<{ reminders: ReminderRow[] }>('/reminders')
      return body.reminders
    },
    enabled,
  })
}

/**
 * Registers a reminder this device computed from its own synced rows.
 *
 * `kind` is only ever `task_due` or `event_soon`: `cards_due` is day-granular and
 * derived from the deck, so the server refuses a client registration for it with
 * a message that says why.
 */
export async function registerReminder(input: {
  kind: 'task_due' | 'event_soon'
  subjectId: string
  fireAt: Date
}): Promise<{ applied: boolean; reminderId: string }> {
  const body = await api.post<{ applied: boolean; reminder_id: string }>('/reminders', {
    kind: input.kind,
    subject_id: input.subjectId,
    fire_at: input.fireAt.toISOString(),
  })
  return { applied: body.applied, reminderId: body.reminder_id }
}

/**
 * Acknowledges one reminder **for this device**.
 *
 * `on conflict do nothing` on the server, so a double tap is a no-op rather than a
 * second delivery row.
 */
export async function ackReminder(id: string): Promise<void> {
  await api.post(`/reminders/${id}/ack`)
}
