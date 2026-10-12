import { describe, expect, it } from 'vitest'

import type { EventRow } from '@/api/queries/events'
import { courseHue, localMonth, localWeek, placeEvents } from './calendar'

/**
 * The calendar's window maths, as pure functions of a fixed clock and an explicit
 * zone.
 *
 * Every case here is a failure that is **invisible on a laptop in Jakarta and
 * wrong in London**: a week boundary computed in the machine's zone, a month grid
 * walked by adding 24 hours, an event placed by its UTC day. The screen renders
 * what these functions return, so a defect here is a defect a student sees and
 * cannot diagnose — a class on the wrong day, a day missing from the month, a
 * timetable that disagrees with the one their faculty published.
 */

/** An event row with only the fields the placement reads, defaulted. */
function event(overrides: Partial<EventRow> & { starts_at: string; ends_at: string }): EventRow {
  return {
    id: 'e1',
    workspace_id: null,
    kind: 'class',
    title: 'Jaringan Komputer',
    source: 'app',
    task_id: null,
    ...overrides,
  }
}

const keys = (days: { key: string }[]) => days.map((day) => day.key)

describe('the calendar window', () => {
  it('is the local week, Monday first, in the user zone', () => {
    // 2026-06-14T17:30:00Z is Sunday the 14th in UTC and **Monday the 15th at
    // 00:30** in Jakarta. The two zones are in different weeks, so a window
    // computed in the machine's zone is a week out for one of them — and the
    // machine running this test is the one that would decide.
    const now = new Date('2026-06-14T17:30:00Z')

    expect(keys(localWeek(now, 'UTC'))).toEqual([
      '2026-06-08',
      '2026-06-09',
      '2026-06-10',
      '2026-06-11',
      '2026-06-12',
      '2026-06-13',
      '2026-06-14',
    ])
    expect(keys(localWeek(now, 'Asia/Jakarta'))).toEqual([
      '2026-06-15',
      '2026-06-16',
      '2026-06-17',
      '2026-06-18',
      '2026-06-19',
      '2026-06-20',
      '2026-06-21',
    ])
  })

  it('starts on Monday even when the local day is a Sunday', () => {
    // The one day of the week where "Monday first" and "the last seven days" differ.
    const sunday = new Date('2026-06-14T06:00:00Z')
    expect(keys(localWeek(sunday, 'UTC'))[0]).toBe('2026-06-08')
    expect(keys(localWeek(sunday, 'UTC'))[6]).toBe('2026-06-14')
  })

  it('covers every day of the local month, including the DST-short one', () => {
    const now = new Date('2026-06-10T18:00:00Z')

    const june = localMonth(now, 'Asia/Jakarta')
    expect(june.filter((cell) => cell.inMonth)).toHaveLength(30)
    expect(keys(june.filter((cell) => cell.inMonth))[0]).toBe('2026-06-01')
    expect(keys(june.filter((cell) => cell.inMonth))[29]).toBe('2026-06-30')
    // A grid is whole weeks, so the cells outside the month are there to fill the
    // first and last rows and are flagged rather than omitted.
    expect(june.length % 7).toBe(0)
    expect(june[0]?.key).toBe('2026-06-01') // June 2026 starts on a Monday
    expect(june[june.length - 1]?.key).toBe('2026-07-05')

    // March 2026 in London holds a 23-hour day (the 29th). Walking the month by
    // adding 24 hours to an instant lands on the wrong calendar date from the
    // transition onwards; walking the calendar does not.
    const march = localMonth(new Date('2026-03-15T12:00:00Z'), 'Europe/London')
    const marchDays = keys(march.filter((cell) => cell.inMonth))
    expect(marchDays).toHaveLength(31)
    expect(marchDays[28]).toBe('2026-03-29')
    expect(marchDays[30]).toBe('2026-03-31')
    expect(new Set(marchDays).size).toBe(31)
  })

  it('covers a month that holds a day the zone skipped entirely', () => {
    // Samoa moved across the date line on 2011-12-29T24:00 local: **2011-12-30
    // does not exist in `Pacific/Apia`**, so no instant in that zone is ever on
    // that date. A month walked by adding 24 hours to an instant therefore has 30
    // cells and is missing a day of December; a month walked as calendar
    // arithmetic has all 31. (Measured: the naive walk yields 2011-12-29 twice
    // and then 2011-12-31.)
    const december = localMonth(new Date('2011-12-15T12:00:00Z'), 'Pacific/Apia')
    const days = keys(december.filter((cell) => cell.inMonth))
    expect(days).toHaveLength(31)
    expect(days[29]).toBe('2011-12-30')
    expect(new Set(days).size).toBe(31)
    expect(december.length % 7).toBe(0)
  })

  it('places a 23:30+07:00 event on the right local day', () => {
    // 2026-06-10T23:30+07:00 is 16:30Z. In Jakarta it is still the 10th; the day
    // after it is the 11th and the event must not appear there.
    const late = event({
      starts_at: '2026-06-10T23:30:00+07:00',
      ends_at: '2026-06-10T23:59:00+07:00',
    })
    const days = localWeek(new Date('2026-06-10T12:00:00Z'), 'Asia/Jakarta')
    const buckets = placeEvents([late], days, 'Asia/Jakarta')

    expect(buckets.find((bucket) => bucket.day.key === '2026-06-10')?.events).toHaveLength(1)
    expect(buckets.find((bucket) => bucket.day.key === '2026-06-11')?.events).toHaveLength(0)
  })

  it('places a 01:00+07:00 event on the local 10th, where UTC puts it on the 9th', () => {
    // The phase's named silent failure: 2026-06-10T01:00+07:00 is 2026-06-09T18:00Z.
    // A UTC implementation shows the class a day early, and nothing on the screen
    // says why.
    const early = event({
      starts_at: '2026-06-10T01:00:00+07:00',
      ends_at: '2026-06-10T03:00:00+07:00',
    })
    const now = new Date('2026-06-10T12:00:00Z')

    const jakarta = placeEvents([early], localWeek(now, 'Asia/Jakarta'), 'Asia/Jakarta')
    expect(jakarta.find((bucket) => bucket.day.key === '2026-06-10')?.events).toHaveLength(1)
    expect(jakarta.find((bucket) => bucket.day.key === '2026-06-09')?.events).toHaveLength(0)

    const utc = placeEvents([early], localWeek(now, 'UTC'), 'UTC')
    expect(utc.find((bucket) => bucket.day.key === '2026-06-09')?.events).toHaveLength(1)
  })

  it('shows an event on every local day it overlaps', () => {
    // `GET /events` returns an *overlap*, so a row that began before the window
    // arrives with the window's own rows. Placing it by its start day alone drops
    // it from the grid with nothing on the page to say where it went.
    const conference = event({
      id: 'e2',
      kind: 'external',
      source: 'ics',
      title: 'Konferensi',
      starts_at: '2026-06-12T20:00:00Z', // 13 June 03:00 in Jakarta
      ends_at: '2026-06-14T10:00:00Z', // 14 June 17:00 in Jakarta
    })
    const days = localWeek(new Date('2026-06-10T12:00:00Z'), 'Asia/Jakarta')
    const buckets = placeEvents([conference], days, 'Asia/Jakarta')

    const on = (key: string) =>
      (buckets.find((bucket) => bucket.day.key === key)?.events ?? []).map((row) => row.id)
    expect(on('2026-06-13')).toEqual(['e2'])
    expect(on('2026-06-14')).toEqual(['e2'])
    expect(on('2026-06-15')).toEqual([])
  })

  it('does not put an event on the day it ends exactly at local midnight', () => {
    // The end is the exclusive edge, which is the server's own rule
    // (`starts_at < to and ends_at > from`) and `day.ts`'s. An event ending at
    // 00:00 on the 14th is not on the 14th.
    const overnight = event({
      starts_at: '2026-06-13T22:00:00+07:00',
      ends_at: '2026-06-14T00:00:00+07:00',
    })
    const days = localWeek(new Date('2026-06-14T12:00:00Z'), 'Asia/Jakarta')
    const buckets = placeEvents([overnight], days, 'Asia/Jakarta')

    expect(buckets.find((bucket) => bucket.day.key === '2026-06-13')?.events).toHaveLength(1)
    expect(buckets.find((bucket) => bucket.day.key === '2026-06-14')?.events).toHaveLength(0)
  })

  it('orders a day chronologically, whatever order the rows arrived in', () => {
    const later = event({ id: 'e-late', starts_at: '2026-06-10T15:00:00Z', ends_at: '2026-06-10T16:00:00Z' })
    const earlier = event({ id: 'e-early', starts_at: '2026-06-10T01:00:00Z', ends_at: '2026-06-10T02:00:00Z' })
    const days = localWeek(new Date('2026-06-10T12:00:00Z'), 'UTC')
    const buckets = placeEvents([later, earlier], days, 'UTC')
    const tenth = buckets.find((bucket) => bucket.day.key === '2026-06-10')

    expect(tenth?.events.map((row) => row.id)).toEqual(['e-early', 'e-late'])
  })

  it('keeps the bucket list aligned with the days it was given', () => {
    const days = localWeek(new Date('2026-06-10T12:00:00Z'), 'Asia/Jakarta')
    const buckets = placeEvents([], days, 'Asia/Jakarta')
    expect(buckets.map((bucket) => bucket.day.key)).toEqual(keys(days))
  })
})

describe('a course hue', () => {
  it('is one of the six, stable for one workspace, and absent without one', () => {
    // No table stores a course colour: the six hues are the spec's categorical
    // palette and the mapping from a workspace to one is derived here, from the
    // id, so it is the same hue on every device and every render. A hue is never
    // the only signal (A233/SPEC § Design): the marker is a 4px bar plus a tinted
    // chip plus the workspace's own initial and name.
    expect(courseHue(null)).toBeNull()
    const first = courseHue('5f0d4b6e-0f6a-4a1a-9d3a-2b6c1e0f9a11')
    expect(first).not.toBeNull()
    expect(first).toBe(courseHue('5f0d4b6e-0f6a-4a1a-9d3a-2b6c1e0f9a11'))
    expect(['indigo', 'sky', 'emerald', 'amber', 'rose', 'violet']).toContain(first)
  })
})
