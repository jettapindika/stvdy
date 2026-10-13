import { describe, expect, it } from 'vitest'

import type { EventRow } from '@/api/queries/events'
import type { SessionRow } from '@/api/queries/study'
import type { TaskRow } from '@/features/tasks/order'
import { dayOffset, todayBuckets, type TodayInput } from './day'

/**
 * The Today view's rules, against a fixed clock.
 *
 * Every case here is one that fails *silently*: a UTC day boundary that puts a
 * Jakarta student's morning task on yesterday's list, a streak off by one, a task
 * shown twice. None of them throws, none of them fails an English test written in
 * the machine's zone, and all of them are wrong for most of this app's users. That
 * is why these are unit tests over pure functions rather than assertions on a
 * screen (A205).
 */

const JAKARTA = 'Asia/Jakarta'

function task(id: string, overrides: Partial<TaskRow> = {}): TaskRow {
  return {
    id,
    workspace_id: null,
    parent_id: null,
    title: `task ${id}`,
    notes: null,
    due_at: null,
    estimate_min: null,
    priority: 2,
    status: 'todo',
    position: 0,
    created_at: '2026-10-01T00:00:00Z',
    updated_at: '2026-10-01T00:00:00Z',
    deleted_at: null,
    ...overrides,
  }
}

function event(id: string, startsAt: string, endsAt: string, kind = 'class'): EventRow {
  return {
    id,
    workspace_id: null,
    kind,
    title: `event ${id}`,
    starts_at: startsAt,
    ends_at: endsAt,
    source: 'app',
    task_id: null,
    created_at: '2026-10-01T00:00:00Z',
    updated_at: '2026-10-01T00:00:00Z',
  }
}

function session(id: string, startedAt: string, durationMin: number): SessionRow {
  return {
    id,
    workspace_id: null,
    task_id: null,
    started_at: startedAt,
    duration_min: durationMin,
  }
}

function input(overrides: Partial<TodayInput> = {}): TodayInput {
  return {
    now: new Date('2026-10-16T01:00:00Z'),
    timeZone: JAKARTA,
    tasks: [],
    events: [],
    sessions: [],
    dueCards: 0,
    activity: { reviews: [], attempts: [], sessions: [] },
    setup: { workspaces: 0, sources: 0, decks: 0 },
    ...overrides,
  }
}

describe('the local day', () => {
  it('places a task in the local day, not the UTC day', () => {
    // 07:00+07:00 on the 16th is 00:00Z on the 16th and *23:00 on the 15th* in
    // UTC. A UTC comparison files it under the 15th, so an Indonesian student
    // sees this morning's task on yesterday's list — the defect this file is for.
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        tasks: [task('morning', { due_at: '2026-10-16T07:00:00+07:00' })],
      }),
    )
    expect(buckets.today.map((entry) => entry.id)).toContain('morning')
    expect(buckets.overdue).toEqual([])
  })

  it('keeps a late-evening task on the same local day', () => {
    // 23:30+07:00 is 16:30Z on the same date, so this case is the one a naive UTC
    // implementation also gets right — which is why both are asserted. A test
    // that only had this one would pass while the morning case failed.
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        tasks: [task('evening', { due_at: '2026-10-16T23:30:00+07:00' })],
      }),
    )
    expect(buckets.today.map((entry) => entry.id)).toContain('evening')
  })

  it('does not list an overdue task as today', () => {
    // A "due today" filter written as a range (`due_at <= end of day`) returns
    // this task in both lists, and the student sees the same task twice with no
    // way to tell which one is the real one.
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        tasks: [
          task('yesterday', { due_at: '2026-10-15T09:00:00+07:00' }),
          task('today', { due_at: '2026-10-16T09:00:00+07:00' }),
        ],
      }),
    )
    expect(buckets.overdue.map((row) => row.id)).toEqual(['yesterday'])
    expect(buckets.today.map((entry) => entry.id)).toEqual(['today'])
  })

  it('leaves a done or tombstoned task out of both lists', () => {
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        tasks: [
          task('done', { due_at: '2026-10-16T09:00:00+07:00', status: 'done' }),
          task('gone', {
            due_at: '2026-10-16T09:00:00+07:00',
            deleted_at: '2026-10-15T00:00:00Z',
          }),
        ],
      }),
    )
    expect(buckets.today).toEqual([])
    expect(buckets.overdue).toEqual([])
  })

  it('puts an undated task in neither list', () => {
    // An undated task has no day to be on. It belongs to the Tasks screen, where
    // the student can give it a date, and putting it on today's timeline would be
    // the app deciding when they must do it.
    const buckets = todayBuckets(input({ tasks: [task('someday')] }))
    expect(buckets.today).toEqual([])
    expect(buckets.overdue).toEqual([])
  })
})

describe('today\u2019s timeline', () => {
  it('orders events and tasks chronologically, not by priority', () => {
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        tasks: [task('high', { due_at: '2026-10-16T21:00:00+07:00', priority: 1 })],
        events: [event('class', '2026-10-16T09:00:00+07:00', '2026-10-16T11:00:00+07:00')],
      }),
    )
    expect(buckets.today.map((entry) => entry.id)).toEqual(['class', 'high'])
  })

  it('keeps an event that started yesterday and runs through today', () => {
    // Overlap, not containment. A three-hour class that began before midnight is
    // on the timeline the student is looking at, and a `starts_at between` filter
    // drops it with nothing on the page to say why.
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        events: [event('overnight', '2026-10-15T23:00:00+07:00', '2026-10-16T02:00:00+07:00')],
      }),
    )
    expect(buckets.today.map((entry) => entry.id)).toEqual(['overnight'])
  })

  it('drops an event that ends exactly when the local day begins', () => {
    // The exclusive edge, and the case a `<=` gets wrong. An event ending at
    // 00:00 on the 16th belongs to the 15th.
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        events: [event('ends-at-midnight', '2026-10-15T22:00:00+07:00', '2026-10-16T00:00:00+07:00')],
      }),
    )
    expect(buckets.today).toEqual([])
  })
})

describe('the week ahead', () => {
  it('lists deadlines and exams in the next seven local days and nothing else', () => {
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        events: [
          event('exam', '2026-10-20T09:00:00+07:00', '2026-10-20T11:00:00+07:00', 'exam'),
          event(
            'deadline',
            '2026-10-16T23:59:00+07:00',
            '2026-10-16T23:59:00+07:00',
            'deadline',
          ),
          // A class is on the timeline but is not a deadline.
          event('class', '2026-10-17T09:00:00+07:00', '2026-10-17T11:00:00+07:00'),
          // The eighth day is outside the window.
          event('far', '2026-10-24T09:00:00+07:00', '2026-10-24T11:00:00+07:00', 'exam'),
          // Yesterday's exam is not ahead of the student.
          event('past', '2026-10-15T09:00:00+07:00', '2026-10-15T11:00:00+07:00', 'exam'),
        ],
      }),
    )
    expect(buckets.thisWeek.map((row) => row.id)).toEqual(['deadline', 'exam'])
  })
})

describe('the streak', () => {
  it('counts a day with any one of a review, an attempt or a session', () => {
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        activity: {
          reviews: ['2026-10-16T09:00:00+07:00'],
          attempts: [],
          sessions: [],
        },
      }),
    )
    expect(buckets.streakDays).toBe(1)
  })

  it('stops the streak at the first missing day', () => {
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        activity: {
          reviews: ['2026-10-16T09:00:00+07:00'],
          attempts: ['2026-10-15T09:00:00+07:00'],
          // The 14th is missing, and the 13th does not bridge it: a streak is
          // consecutive days, not a count of active ones.
          sessions: ['2026-10-13T09:00:00+07:00'],
        },
      }),
    )
    expect(buckets.streakDays).toBe(2)
  })

  it('counts today even when it is not over yet', () => {
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        activity: { reviews: ['2026-10-16T07:30:00+07:00'], attempts: [], sessions: [] },
      }),
    )
    expect(buckets.streakDays).toBe(1)
  })

  it('keeps the streak when today is still empty, and reports zero after a gap', () => {
    // A student who has not opened the app yet today has not lost their streak;
    // reporting zero at 09:00 would be the app taking away something they earned.
    const kept = todayBuckets(
      input({
        now: new Date('2026-10-16T02:00:00Z'),
        activity: { reviews: ['2026-10-15T09:00:00+07:00'], attempts: [], sessions: [] },
      }),
    )
    expect(kept.streakDays).toBe(1)

    const lost = todayBuckets(
      input({
        now: new Date('2026-10-16T02:00:00Z'),
        activity: { reviews: ['2026-10-13T09:00:00+07:00'], attempts: [], sessions: [] },
      }),
    )
    expect(lost.streakDays).toBe(0)
  })

  it('counts the local day, so a 23:30 session and a 00:30 review are two days', () => {
    // In UTC both are on the 16th; in Jakarta they are the 16th and the 17th. A
    // UTC streak merges two days of study into one and breaks elsewhere.
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-17T01:00:00Z'),
        activity: {
          reviews: ['2026-10-17T00:30:00+07:00'],
          attempts: [],
          sessions: ['2026-10-16T23:30:00+07:00'],
        },
      }),
    )
    expect(buckets.streakDays).toBe(2)
  })
})

describe('the streak strip and the week it sits beside', () => {
  it('draws five local days ending today, oldest first', () => {
    // The strip and the streak have to answer one question, so the last cell is
    // today and the days are the local ones: a UTC walk would end on the 15th for
    // a Jakarta student at 08:00 on the 16th.
    const buckets = todayBuckets(
      input({ now: new Date('2026-10-16T01:00:00Z') }),
    )
    expect(buckets.recentDays.map((day) => day.key)).toEqual([
      '2026-10-12',
      '2026-10-13',
      '2026-10-14',
      '2026-10-15',
      '2026-10-16',
    ])
  })

  it('marks a day active on the same rule the streak counts with', () => {
    // Three sources, one rule: a day counts if it holds any one of a review, an
    // attempt or a session. The strip is filled from that same set, so it cannot
    // show a filled day inside a streak that stopped.
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        activity: {
          reviews: ['2026-10-16T09:00:00+07:00'],
          attempts: ['2026-10-14T09:00:00+07:00'],
          sessions: ['2026-10-12T09:00:00+07:00'],
        },
      }),
    )
    expect(buckets.recentDays.filter((day) => day.active).map((day) => day.key)).toEqual([
      '2026-10-12',
      '2026-10-14',
      '2026-10-16',
    ])
    expect(buckets.streakDays).toBe(1)
  })

  it('reports the seven local days the week ahead covers', () => {
    const buckets = todayBuckets(input({ now: new Date('2026-10-16T01:00:00Z') }))
    expect(buckets.week).toEqual({ from: '2026-10-16', to: '2026-10-22' })
  })

  it('measures a deadline in calendar days, not in elapsed hours', () => {
    // 23:59 and 00:01 are two minutes apart and one calendar day apart, and the
    // label a student reads ("In 2 days") has to be the calendar one.
    expect(dayOffset('2026-10-16', '2026-10-18', 6)).toBe(2)
    expect(dayOffset('2026-10-16', '2026-10-16', 6)).toBe(0)
    // Outside the window there is no label, rather than a negative or a guess.
    expect(dayOffset('2026-10-16', '2026-10-23', 6)).toBeNull()
    expect(dayOffset('2026-10-16', '2026-10-15', 6)).toBeNull()
  })
})

describe('the numbers on the screen', () => {
  it('sums only the minutes actually logged today', () => {
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T01:00:00Z'),
        sessions: [
          session('a', '2026-10-16T09:00:00+07:00', 25),
          session('b', '2026-10-16T14:00:00+07:00', 50),
          session('yesterday', '2026-10-15T09:00:00+07:00', 90),
        ],
      }),
    )
    expect(buckets.minutesToday).toBe(75)
  })

  it('counts a session in the first half-hour of the local day, while UTC is still yesterday', () => {
    // 00:30+07:00 on the 17th is 17:30Z on the *16th*. A UTC day boundary puts
    // these minutes on yesterday and shows the student zero at 00:30, which is
    // exactly when a late-night session gets logged.
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T17:00:00Z'),
        sessions: [session('late', '2026-10-17T00:30:00+07:00', 30)],
      }),
    )
    expect(buckets.minutesToday).toBe(30)
  })

  it('does not leak yesterday\u2019s late session into today', () => {
    // The other side of the same boundary: at 00:00 Jakarta on the 17th, a
    // 23:30 session from the 16th is yesterday's. UTC still reads the 16th, so a
    // UTC filter would keep counting it all through the local morning.
    const buckets = todayBuckets(
      input({
        now: new Date('2026-10-16T17:00:00Z'),
        sessions: [session('yesterday', '2026-10-16T23:30:00+07:00', 45)],
      }),
    )
    expect(buckets.minutesToday).toBe(0)
  })

  it('reports zeros rather than placeholders', () => {
    // No data means no numbers, never a plausible-looking default. The setup
    // counts are echoed because the new-user checklist is the one place the
    // screen shows three numbers that are not about today.
    const buckets = todayBuckets(input())
    expect(buckets.today).toEqual([])
    expect(buckets.overdue).toEqual([])
    expect(buckets.thisWeek).toEqual([])
    expect(buckets.minutesToday).toBe(0)
    expect(buckets.streakDays).toBe(0)
    expect(buckets.dueCards).toBe(0)
    expect(buckets.setup).toEqual({ workspaces: 0, sources: 0, decks: 0 })
  })

  it('passes the due-card count through unchanged', () => {
    expect(todayBuckets(input({ dueCards: 7 })).dueCards).toBe(7)
  })
})
