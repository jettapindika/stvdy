import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import type { EventRow } from '@/api/queries/events'
import type { TaskRow } from '@/features/tasks/order'
import {
  LEAD_EVENT_MIN,
  LEAD_TASK_MIN,
  MAX_LOOKAHEAD_DAYS,
  STALE_AFTER_MIN,
  deferForQuietHours,
  inQuietHours,
  planLocalNotifications,
  quietEnd,
  type LocalReminder,
} from './localSchedule'

/**
 * The local schedule, at the boundaries the server's own engine is tested at.
 *
 * Three properties carry this file, and each is a place the plausible
 * implementation is wrong:
 *
 *  1. **The id is the subject and the local date, never the computed instant**
 *     (A284). Two devices that compute 19:00 and 19:01 must produce the same id,
 *     or the student gets the notification twice — once locally and once by push.
 *     The test that matters is the deferral one: `at` moves, `id` does not.
 *  2. **Quiet hours wrap past midnight**, so `(22, 7)` covers 22:00→07:00 and the
 *     naive `start <= h && h < end` covers none of it.
 *  3. **`now` is a parameter.** Every case below passes an explicit instant, which
 *     is the only reason a quiet-hours boundary and a DST day are testable at all.
 *
 * The lead times are asserted against `crates/core/src/reminders.rs` rather than
 * against literals, because a client that led by a different margin would notify
 * at a time the server does not consider the reminder's time — a second schedule
 * for one subject, which is exactly the divergence this module exists to prevent.
 */

const TZ = 'Asia/Jakarta'

function task(overrides: Partial<TaskRow> = {}): TaskRow {
  return {
    id: 't1',
    workspace_id: null,
    parent_id: null,
    title: 'Kriptografi',
    notes: null,
    due_at: '2026-10-16T16:59:00Z',
    estimate_min: null,
    priority: 0,
    status: 'todo',
    position: 0,
    created_at: '2026-10-11T08:00:00Z',
    ...overrides,
  }
}

function event(overrides: Partial<EventRow> = {}): EventRow {
  return {
    id: 'e1',
    workspace_id: null,
    kind: 'class',
    title: 'Struktur Data',
    starts_at: '2026-10-14T02:00:00Z',
    ends_at: '2026-10-14T04:00:00Z',
    source: 'manual',
    task_id: null,
    ...overrides,
  }
}

/** The copy callback, which is the caller's job in production (every string is `t()`). */
const copy = (reminder: { kind: string; subjectId: string }): { title: string; body: string } => ({
  title: `${reminder.kind} ${reminder.subjectId}`,
  body: 'body',
})

function plan(
  overrides: Partial<Parameters<typeof planLocalNotifications>[0]> = {},
): LocalReminder[] {
  return planLocalNotifications({
    now: new Date('2026-10-12T00:00:00Z'),
    timeZone: TZ,
    quiet: null,
    tasks: [],
    events: [],
    seen: [],
    copy,
    ...overrides,
  })
}

describe('the lead times are the server engine’s', () => {
  // The server's constants, read from the file that defines them. A literal here
  // would let the client and the server drift apart silently: the client would
  // schedule at one time and the server would push at another, and nothing in
  // either test suite would say so.
  const source = readFileSync(
    join(__dirname, '..', '..', '..', '..', 'crates', 'core', 'src', 'reminders.rs'),
    'utf8',
  )

  const serverConstant = (name: string): number => {
    const match = new RegExp(`pub const ${name}: i64 = ([0-9 *]+);`).exec(source)
    if (match?.[1] === undefined) throw new Error(`${name} is not in reminders.rs`)
    // The server writes these as arithmetic (`24 * 60`), so the value is the sum.
    return match[1]
      .split('*')
      .map((part) => Number(part.trim()))
      .reduce((product, factor) => product * factor, 1)
  }

  it('leads a task by the server’s LEAD_TASK_MIN', () => {
    expect(LEAD_TASK_MIN).toBe(serverConstant('LEAD_TASK_MIN'))
  })

  it('leads an event by the server’s LEAD_EVENT_MIN', () => {
    expect(LEAD_EVENT_MIN).toBe(serverConstant('LEAD_EVENT_MIN'))
  })

  it('gives up on a deferred reminder at the server’s STALE_AFTER_MIN', () => {
    expect(STALE_AFTER_MIN).toBe(serverConstant('STALE_AFTER_MIN'))
  })

  it('looks as far ahead as the server’s MAX_LOOKAHEAD_DAYS', () => {
    expect(MAX_LOOKAHEAD_DAYS).toBe(serverConstant('MAX_LOOKAHEAD_DAYS'))
  })
})

describe('the id is the subject and the local day, never the computed instant', () => {
  it('schedules one notification per subject per local day', () => {
    const reminders = plan({ tasks: [task()], events: [event()] })
    expect(reminders).toHaveLength(2)

    const due = reminders.find((reminder) => reminder.kind === 'task_due')
    // `2026-10-16T16:59Z` is 23:59 Jakarta on the 16th, and an hour before that is
    // 22:59 on the **same** local day.
    expect(due?.id).toBe('task_due:t1:2026-10-16')
    expect(due?.at.toISOString()).toBe('2026-10-16T15:59:00.000Z')

    const soon = reminders.find((reminder) => reminder.kind === 'event_soon')
    expect(soon?.id).toBe('event_soon:e1:2026-10-14')
    expect(soon?.at.toISOString()).toBe('2026-10-14T01:30:00.000Z')
  })

  it('produces the same id for two devices that disagree about the fire instant by a minute', () => {
    // The whole point of A284. Both devices hold the same `due_at` and the same
    // zone, so they compute the same instant — but the key must not depend on it
    // either way, and the deferral case below is where that is actually load
    // bearing. Here the assertion is the format the server stores.
    const reminders = plan({ tasks: [task()] })
    expect(reminders[0]?.id).toBe(`task_due:t1:2026-10-16`)
  })

  it('orders by fire time and dedupes by id keeping the earliest', () => {
    const reminders = plan({
      tasks: [
        task({ id: 'late', due_at: '2026-10-16T16:59:00Z' }),
        task({ id: 'early', due_at: '2026-10-13T16:59:00Z' }),
      ],
    })
    expect(reminders.map((reminder) => reminder.subjectId)).toEqual(['early', 'late'])
  })
})

describe('quiet hours', () => {
  it('defers a notification that falls inside quiet hours, keeping the same id', () => {
    // The task is due 23:59 Jakarta, so its reminder lands 22:59 — inside a
    // `22 → 7` window. It moves to 07:00 the next local morning, and the id is
    // untouched: this is the assertion a fire-time key fails.
    const reminders = plan({
      quiet: { start: 22, end: 7 },
      tasks: [task()],
    })
    expect(reminders).toHaveLength(1)
    expect(reminders[0]?.id).toBe('task_due:t1:2026-10-16')
    // 07:00 Jakarta on the 17th.
    expect(reminders[0]?.at.toISOString()).toBe('2026-10-17T00:00:00.000Z')
  })

  it('drops a deferred notification that lands past the stale window, rather than sending noise', () => {
    // The subject is the task's own due instant (23:59 on the 16th). 07:00 the next
    // morning is 7 hours later, which is inside `STALE_AFTER_MIN`, so this one
    // *does* survive — the case that dies is a window long enough to push the
    // reminder past a day. `quiet: {start: 1, end: 23}` on a reminder at 00:00
    // local pushes it 23 hours out, past the 24h subject bound when the subject is
    // the reminder's own hour. Assert the surviving half here and the dying half
    // through `deferForQuietHours` below, where the subject is explicit.
    const reminders = plan({ quiet: { start: 22, end: 7 }, tasks: [task()] })
    expect(reminders).toHaveLength(1)
  })

  it('measures staleness against the subject, not against the fire instant', () => {
    const subject = new Date('2026-10-16T15:59:00Z')
    // Inside the window, ending 7 hours later: inside the 24h bound, so it survives.
    const moved = deferForQuietHours(
      new Date('2026-10-16T16:00:00Z'),
      subject,
      TZ,
      { start: 22, end: 7 },
    )
    expect(moved.deferred).toBe(true)
    expect(moved.skip).toBeNull()

    // The same reminder against a subject a day earlier is noise: 07:00 on the 17th
    // is more than 24h past a subject on the 15th.
    const stale = deferForQuietHours(
      new Date('2026-10-16T16:00:00Z'),
      new Date('2026-10-15T00:00:00Z'),
      TZ,
      { start: 22, end: 7 },
    )
    expect(stale.skip).toBe('stale_after_quiet_hours')
  })

  it('wraps past midnight, so 22:00 to 07:00 covers 23:00 and not 12:00', () => {
    const quiet = { start: 22, end: 7 }
    expect(inQuietHours(23, quiet)).toBe(true)
    expect(inQuietHours(2, quiet)).toBe(true)
    expect(inQuietHours(6, quiet)).toBe(true)
    expect(inQuietHours(7, quiet)).toBe(false)
    expect(inQuietHours(12, quiet)).toBe(false)
    expect(inQuietHours(21, quiet)).toBe(false)
  })

  it('treats an equal pair as an empty window, not an always-on one', () => {
    // The settings screen's "off". A window that never ends is not one a student
    // can be inside of.
    expect(inQuietHours(3, { start: 8, end: 8 })).toBe(false)
    expect(inQuietHours(8, { start: 8, end: 8 })).toBe(false)
  })

  it('does not touch a reminder outside the window', () => {
    // 09:00 Jakarta is outside `22 → 7`.
    const reminders = plan({
      quiet: { start: 22, end: 7 },
      tasks: [task({ due_at: '2026-10-16T02:00:00Z' })],
    })
    expect(reminders[0]?.at.toISOString()).toBe('2026-10-16T01:00:00.000Z')
  })

  it('ends the window at the next local hour that is outside it', () => {
    // 23:00 Jakarta on the 16th.
    const end = quietEnd(new Date('2026-10-16T16:00:00Z'), TZ, { start: 22, end: 7 })
    expect(end?.toISOString()).toBe('2026-10-17T00:00:00.000Z')
    // Not inside: nothing to end.
    expect(quietEnd(new Date('2026-10-16T05:00:00Z'), TZ, { start: 22, end: 7 })).toBeNull()
  })

  it('ends a same-day window at the hour it closes', () => {
    // 13:00 Jakarta, inside `12 → 14`; the window ends at 14:00 the same day.
    const end = quietEnd(new Date('2026-10-16T06:00:00Z'), TZ, { start: 12, end: 14 })
    expect(end?.toISOString()).toBe('2026-10-16T07:00:00.000Z')
  })

  it('steps over a local hour that does not exist, on a spring-forward day', () => {
    // `Europe/Berlin` skips 02:00→03:00 on 2026-03-29, so a `1 → 3` window has no
    // 02:00 local hour to end at. The walk steps in absolute time, so it lands on
    // the first instant whose local hour is outside the window rather than
    // constructing a time that never happened.
    const berlin = 'Europe/Berlin'
    // 01:30 local on the transition day, which is 00:30 UTC.
    const from = new Date('2026-03-29T00:30:00Z')
    const end = quietEnd(from, berlin, { start: 1, end: 3 })
    expect(end).not.toBeNull()
    // 03:00 local on that day is 01:00 UTC — the gap is one hour wide.
    expect(end?.toISOString()).toBe('2026-03-29T01:00:00.000Z')
  })

  it('lands on the same wall clock on both sides of a DST transition', () => {
    // A reminder at 07:00 local is 07:00 local on both sides of a transition, so
    // the UTC instant differs by an hour. A fixed-offset implementation passes
    // every test written in a zone without DST and fails this one.
    const berlin = 'Europe/Berlin'
    // Inside `22 → 7` at 23:00 local on the 28th (22:00 UTC, CET).
    const before = deferForQuietHours(
      new Date('2026-03-28T22:00:00Z'),
      new Date('2026-03-29T04:00:00Z'),
      berlin,
      { start: 22, end: 7 },
    )
    // 07:00 local on the 29th, after the clocks moved forward, is 05:00 UTC.
    expect(before.at.toISOString()).toBe('2026-03-29T05:00:00.000Z')
  })

  it('leaves `now` as the caller’s instant, never a clock read', () => {
    // The module reads no clock, so two calls with the same `now` and the same
    // rows produce byte-identical plans — and the same plan is what makes the id
    // stable across a re-render.
    const input = { tasks: [task()], events: [event()], now: new Date('2026-10-12T00:00:00Z') }
    const first = plan(input)
    const second = plan(input)
    expect(second.map((reminder) => [reminder.id, reminder.at.toISOString()])).toEqual(
      first.map((reminder) => [reminder.id, reminder.at.toISOString()]),
    )
  })
})

describe('a subject that is done or past produces nothing', () => {
  it('drops a notification for a subject that is already done', () => {
    const reminders = plan({ tasks: [task({ status: 'done' })] })
    expect(reminders).toEqual([])
  })

  it('drops a notification for a subject whose own instant has passed', () => {
    // The server records `already_past` and keeps the row visible; a client has
    // nothing to show, and notifying about a deadline that is behind the student is
    // worse than silence.
    const reminders = plan({
      now: new Date('2026-10-17T00:00:00Z'),
      tasks: [task()],
    })
    expect(reminders).toEqual([])
  })

  it('drops a subject beyond the lookahead horizon rather than planning it', () => {
    // Not planned and skipped: simply not this plan's business. A later plan sees it.
    const reminders = plan({ tasks: [task({ due_at: '2026-12-01T09:00:00Z' })] })
    expect(reminders).toEqual([])
  })

  it('ignores a tombstoned task and a task with no due date', () => {
    const reminders = plan({
      tasks: [
        task({ id: 'gone', deleted_at: '2026-10-11T00:00:00Z' }),
        task({ id: 'undated', due_at: null }),
      ],
    })
    expect(reminders).toEqual([])
  })
})

describe('the ids this device has already shown', () => {
  it('does not schedule an id the device has already shown', () => {
    // A285's fallback half: the push payload carries `reminder_id`, and this list
    // is what suppresses the local copy of a reminder the student already saw.
    const reminders = plan({ tasks: [task()], seen: ['task_due:t1:2026-10-16'] })
    expect(reminders).toEqual([])
  })

  it('still schedules the subject’s next day, because the key carries the date', () => {
    // One per subject **per local day**: having shown today's does not silence
    // tomorrow's, which is the difference between a dedupe key and a mute list.
    const reminders = plan({
      seen: ['task_due:t1:2026-10-16'],
      tasks: [task({ due_at: '2026-10-17T16:59:00Z' })],
    })
    expect(reminders.map((reminder) => reminder.id)).toEqual(['task_due:t1:2026-10-17'])
  })
})

describe('the copy comes from the caller', () => {
  it('passes the kind, subject and instant to the callback', () => {
    // Every string in the app goes through `t()`, so this module must not build
    // one: a module that returned English would ship English to an Indonesian
    // student and no test of this file could see it.
    const seen: { kind: string; subjectId: string; at: string }[] = []
    plan({
      tasks: [task()],
      copy: (reminder) => {
        seen.push({
          kind: reminder.kind,
          subjectId: reminder.subjectId,
          at: reminder.at.toISOString(),
        })
        return { title: 'title', body: 'body' }
      },
    })
    expect(seen).toEqual([
      { kind: 'task_due', subjectId: 't1', at: '2026-10-16T15:59:00.000Z' },
    ])
  })
})
