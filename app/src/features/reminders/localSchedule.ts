import { localDateKey } from '@/lib/time'
import type { EventRow } from '@/api/queries/events'
import type { TaskRow } from '@/features/tasks/order'

/**
 * The local notifications this device schedules from its own synced rows.
 *
 * Pure, and `now` is a parameter rather than a call. That is not a style
 * preference: a clock read inside is the one change no behavioural test has to
 * notice, and it is what makes this testable at a quiet-hours boundary and across
 * a DST day at all. `crates/core/src/reminders.rs` follows the same discipline for
 * the same reason, and its purity sweep enforces it there.
 *
 * **The rule is the server's rule, and it is one rule.** A client that schedules
 * a notification the server also pushes is the double-notification the
 * cross-device dedupe exists to prevent, so this mirrors
 * `crates/core/src/reminders.rs` exactly:
 *
 *  * **one per subject per local day** — the id is
 *    `"<kind>:<subject_id>:<local date>"`, which is `reminders::dedupe_key`'s
 *    format, and it is derived from the subject and the student's local date and
 *    **never from the computed instant** (A284). Two devices that compute 19:00
 *    and 19:01 produce the same id, and a quiet-hours deferral moves `at` while
 *    leaving the id alone. An id built from the fire instant fires once per
 *    device.
 *  * **quiet hours defer rather than drop** — a fire time inside the window moves
 *    to the window's end without changing its id (A286). A reminder that silently
 *    never arrives is what a student reports as "reminders are broken".
 *  * **a done subject produces none** — a completed task is not a deadline.
 *
 * The lead times are the server's, and `localSchedule.test.ts` reads
 * `crates/core/src/reminders.rs` and asserts they match: a client that led by a
 * different margin would notify at a time the server does not consider the
 * reminder's time, which is a second schedule for the same subject.
 *
 * **`cards_due` is deliberately absent.** It is day-granular, derived from the
 * deck, and the server's own route refuses a client registration for it — the
 * client has nothing to compute it from that the server does not already have. So
 * this plans the two subject kinds the client can compute, and the day's card
 * reminder arrives over push.
 */

/** How far before a task's due time its reminder fires. Mirrors `LEAD_TASK_MIN`. */
export const LEAD_TASK_MIN = 60

/** How far before an event's start time its reminder fires. Mirrors `LEAD_EVENT_MIN`. */
export const LEAD_EVENT_MIN = 30

/**
 * After this, a deferred reminder is not worth sending. Mirrors `STALE_AFTER_MIN`.
 *
 * Measured against the *subject*, not against the fire instant: a task due at
 * 09:00 reminded at 07:00 the next morning is noise, not a reminder.
 */
export const STALE_AFTER_MIN = 24 * 60

/** The furthest ahead a reminder may be scheduled. Mirrors `MAX_LOOKAHEAD_DAYS`. */
export const MAX_LOOKAHEAD_DAYS = 14

/** The two kinds this module plans. The server's third (`cards_due`) is push-only. */
export type LocalReminderKind = 'task_due' | 'event_soon'

/**
 * One notification to schedule.
 *
 * `id` is the dedupe key, and it is the same string the server stores in
 * `reminders.dedupe_key` for the same subject and local day. That identity is what
 * makes the `seen` list work as the cross-device fallback: a push carrying
 * `reminder_id` is suppressed when this device already showed that id (A285).
 */
export interface LocalReminder {
  /** `"<kind>:<subject_id>:<YYYY-MM-DD>"`, the server's key for the same reminder. */
  id: string
  kind: LocalReminderKind
  /** The task's or event's id, which is what `POST /reminders` takes. */
  subjectId: string
  title: string
  body: string
  /** When to show it, after any quiet-hours deferral. */
  at: Date
}

/** The copy a notification shows, resolved in the reader's language by the caller. */
export interface LocalReminderCopy {
  title: string
  body: string
}

export interface LocalScheduleInput {
  /** The instant to plan from. A parameter, never a clock read inside. */
  now: Date
  /** The student's IANA zone, from the account (`users.timezone`), never the device's. */
  timeZone: string
  /** `(start_hour, end_hour)` in the student's local zone, or null for none. */
  quiet: { start: number; end: number } | null
  tasks: TaskRow[]
  events: EventRow[]
  /**
   * The ids this device has already shown, from `Platform.localState`.
   *
   * The mechanism A285's fallback half names: a reminder the student already saw
   * — as a local notification, or as a push that arrived first — is not shown
   * again, on this device.
   */
  seen: string[]
  /**
   * Resolves the notification's copy.
   *
   * A callback rather than strings built here, because this module is pure and
   * every string in the app goes through `t()`: a module that returned English
   * would ship English to an Indonesian student, and no test of this file could
   * see it.
   */
  copy: (reminder: { kind: LocalReminderKind; subjectId: string; at: Date }) => LocalReminderCopy
}

/**
 * The notifications to schedule, in fire order.
 *
 * Sorted by `(at, kind, subject id)` and deduped by id keeping the earliest, which
 * is `reminders::plan`'s order and its dedupe rule: a plan whose order or whose
 * keys moved between two runs is a reminder that fires twice on one device and
 * never on another.
 */
export function planLocalNotifications(input: LocalScheduleInput): LocalReminder[] {
  const { now, timeZone, quiet, seen } = input
  const seenIds = new Set(seen)
  const horizon = new Date(now.getTime() + MAX_LOOKAHEAD_DAYS * 24 * 60 * 60 * 1000)

  const planned: LocalReminder[] = []

  const consider = (subject: {
    kind: LocalReminderKind
    id: string
    /** The instant the reminder is *about*: the due time or the start. */
    at: string | null
    done: boolean
    leadMin: number
  }) => {
    if (subject.done) return
    if (subject.at === null) return
    const subjectAt = new Date(subject.at)
    if (Number.isNaN(subjectAt.getTime())) return

    const fireAt = new Date(subjectAt.getTime() - subject.leadMin * 60 * 1000)
    if (fireAt.getTime() > horizon.getTime()) return

    // The subject's own instant has already passed. The server records this as
    // `already_past` and keeps the row; a client has nothing to show for it, and a
    // notification for a deadline that is already behind the student is worse than
    // silence.
    if (subjectAt.getTime() <= now.getTime()) return

    const localDate = localDateKey(fireAt, timeZone)
    const id = `${subject.kind}:${subject.id}:${localDate}`
    if (seenIds.has(id)) return

    const deferral = deferForQuietHours(fireAt, subjectAt, timeZone, quiet)
    if (deferral.skip !== null) return

    const text = input.copy({ kind: subject.kind, subjectId: subject.id, at: deferral.at })
    planned.push({
      id,
      kind: subject.kind,
      subjectId: subject.id,
      title: text.title,
      body: text.body,
      at: deferral.at,
    })
  }

  for (const task of input.tasks) {
    if (task.deleted_at != null) continue
    consider({
      kind: 'task_due',
      id: task.id,
      at: task.due_at,
      done: task.status === 'done',
      leadMin: LEAD_TASK_MIN,
    })
  }

  for (const event of input.events) {
    consider({
      kind: 'event_soon',
      id: event.id,
      at: event.starts_at,
      done: false,
      leadMin: LEAD_EVENT_MIN,
    })
  }

  planned.sort(
    (left, right) =>
      left.at.getTime() - right.at.getTime() ||
      left.kind.localeCompare(right.kind) ||
      left.subjectId.localeCompare(right.subjectId),
  )

  // Deduped by id, keeping the first of the sorted order — which is the earliest
  // fire time for that id. Two inputs can share an id only when a task and an event
  // share both a kind and an id, which cannot happen; the pass is here because the
  // server's own plan does it, and a client that skipped it would diverge the first
  // time either side gained a subject source.
  const kept: LocalReminder[] = []
  const byId = new Set<string>()
  for (const reminder of planned) {
    if (byId.has(reminder.id)) continue
    byId.add(reminder.id)
    kept.push(reminder)
  }
  return kept
}

/** Where a reminder lands, whether quiet hours moved it, and whether the move killed it. */
export interface Deferral {
  at: Date
  deferred: boolean
  skip: 'stale_after_quiet_hours' | null
}

/**
 * Moves a fire time out of the quiet window, or says the move makes it worthless.
 *
 * The same rule as `reminders::defer_for_quiet_hours`, and the same signature
 * shape: `subjectAt` is what `STALE_AFTER_MIN` is measured against, so this answers
 * "is this still about the thing the student asked to be reminded of".
 */
export function deferForQuietHours(
  fireAt: Date,
  subjectAt: Date,
  timeZone: string,
  quiet: { start: number; end: number } | null,
): Deferral {
  if (quiet === null) return { at: fireAt, deferred: false, skip: null }
  if (!inQuietHours(localParts(fireAt, timeZone).hour, quiet)) {
    return { at: fireAt, deferred: false, skip: null }
  }
  const end = quietEnd(fireAt, timeZone, quiet)
  if (end === null) return { at: fireAt, deferred: false, skip: null }
  const stale = end.getTime() > subjectAt.getTime() + STALE_AFTER_MIN * 60 * 1000
  return { at: end, deferred: true, skip: stale ? 'stale_after_quiet_hours' : null }
}

/**
 * Wrap-midnight membership, the same semantics as A202 and as `in_quiet_hours`.
 *
 * `(22, 7)` is 22:00→07:00: every hour from 22 through 6 is inside, and the naive
 * `start <= hour && hour < end` is false for all of them. `start === end` is an
 * empty window rather than an always-on one — the settings screen's "off".
 */
export function inQuietHours(localHourValue: number, quiet: { start: number; end: number }): boolean {
  const { start, end } = quiet
  if (start === end) return false
  if (start < end) return localHourValue >= start && localHourValue < end
  return localHourValue >= start || localHourValue < end
}

/**
 * The instant a quiet window ends, from inside it.
 *
 * `null` when `from` is not inside the window: there is nothing to end.
 *
 * The walk steps one hour at a time from the top of the local hour containing
 * `from`, which is what makes it correct on both sides of a transition — a
 * spring-forward hour that does not exist is stepped over, and a fall-back hour
 * that happens twice stops at the first occurrence at or after `from`. This is
 * `reminders::quiet_end`'s walk, and the stepping is in absolute time rather than
 * in wall-clock arithmetic, so no missing hour is ever constructed.
 */
export function quietEnd(
  from: Date,
  timeZone: string,
  quiet: { start: number; end: number },
): Date | null {
  const local = localParts(from, timeZone)
  if (!inQuietHours(local.hour, quiet)) return null

  // The top of the local hour containing `from`. Subtracting the local minute and
  // second is exact for every real zone: an IANA offset is always a whole number of
  // minutes, so a minute of local time is a minute of absolute time.
  let cursor =
    from.getTime() - local.minute * 60_000 - local.second * 1000 - from.getMilliseconds()

  for (let step = 0; step < 48; step += 1) {
    cursor += 60 * 60 * 1000
    const at = localParts(new Date(cursor), timeZone)
    if (cursor > from.getTime() && !inQuietHours(at.hour, quiet)) return new Date(cursor)
  }
  return null
}

interface LocalParts {
  year: number
  month: number
  day: number
  hour: number
  minute: number
  second: number
}

const partCache = new Map<string, Intl.DateTimeFormat>()

function formatter(timeZone: string): Intl.DateTimeFormat {
  const cached = partCache.get(timeZone)
  if (cached !== undefined) return cached
  const created = new Intl.DateTimeFormat('en-GB', {
    timeZone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
    second: '2-digit',
    // `h23` rather than `hour12: false`: some locales render midnight as hour 24
    // with `hour12: false`, which would put 00:30 outside a `(22, 7)` window.
    hourCycle: 'h23',
  })
  partCache.set(timeZone, created)
  return created
}

/** The local wall-clock fields of an instant, in the given zone. */
function localParts(instant: Date, timeZone: string): LocalParts {
  const parts = formatter(timeZone).formatToParts(instant)
  const at = (type: Intl.DateTimeFormatPartTypes): number =>
    Number(parts.find((part) => part.type === type)?.value ?? '0')
  return {
    year: at('year'),
    month: at('month'),
    day: at('day'),
    hour: at('hour'),
    minute: at('minute'),
    second: at('second'),
  }
}
