import type { EventRow } from '@/api/queries/events'
import { formatInZone, localDateKey } from '@/lib/time'
import { COURSE_NAMES, type CourseName } from '@/tokens/tokens'

/**
 * The calendar's window maths, as pure functions.
 *
 * The screen renders exactly what these return, so the failures worth preventing
 * are the ones nobody can see from a laptop: a week boundary computed in the
 * machine's zone (right in Jakarta, a week out in London), a month grid walked by
 * adding 24 hours to an instant (missing a day in a zone with a DST jump, and
 * missing one entirely in a zone that skipped a date), an event placed by its UTC
 * day (a 06:00 class on the previous day's row).
 *
 * Every zone comes from `Intl.DateTimeFormat` with an explicit `timeZone` and
 * never from the machine, which is `lib/time.ts`'s rule and the reason
 * `localDateKey` is used rather than `Date#getDate`.
 *
 * The month is walked as **calendar arithmetic** — `addDays` on a `YYYY-MM-DD`
 * key — and not by adding 24 hours to an instant. The two differ on every DST
 * boundary, and the difference is a missing or duplicated day. `day.ts` states the
 * same rule for the same reason; this is the grid's copy of it, over the range a
 * month grid needs rather than a week's.
 *
 * Nothing here reads a clock or a fetch: `now` is a parameter, which is what makes
 * the whole file testable against a fixed instant and what stops the screen from
 * showing a day the app did not compute (gate 7a).
 */

/** One cell of a grid: the local calendar date, and whether it is in the month. */
export interface Day {
  /** `YYYY-MM-DD` in the user's zone. */
  key: string
  /** False for the leading and trailing cells a whole-week grid needs. */
  inMonth: boolean
}

/** A day and the events on it, in the order the grid renders them. */
export interface DayBucket {
  day: Day
  events: EventRow[]
}

const WEEK_LENGTH = 7

/**
 * The calendar date `days` after `key`.
 *
 * Pure integer arithmetic on the calendar, deliberately not `key + 24h`. `Date.UTC`
 * normalises the overflow and the ISO prefix is the date it normalised to, so this
 * is exact for any offset change and for a date a zone never had.
 */
function addDays(key: string, days: number): string {
  const [year, month, day] = key.split('-').map(Number)
  return new Date(Date.UTC(year ?? 0, (month ?? 1) - 1, (day ?? 1) + days))
    .toISOString()
    .slice(0, 10)
}

/** The day of the week `key` is, 0 = Sunday, without going through an instant. */
function weekdayOf(key: string): number {
  const [year, month, day] = key.split('-').map(Number)
  return new Date(Date.UTC(year ?? 0, (month ?? 1) - 1, day ?? 1)).getUTCDay()
}

/** `YYYY-MM-DD` keys, `count` of them, starting at `from`. */
function range(from: string, count: number): string[] {
  return Array.from({ length: count }, (_, index) => addDays(from, index))
}

/**
 * The local week containing `now`: **Monday first**, seven days, in the user's
 * zone.
 *
 * Monday rather than Sunday because the spec's grid is the Indonesian academic
 * week, and because a week that starts on a different day is a week whose rows do
 * not line up with the timetable the student was given.
 */
export function localWeek(now: Date, zone: string): Day[] {
  const today = localDateKey(now, zone)
  // `getUTCDay` is 0 for Sunday; the Monday-first index is `(day + 6) % 7`.
  const back = (weekdayOf(today) + 6) % WEEK_LENGTH
  return range(addDays(today, -back), WEEK_LENGTH).map((key) => ({ key, inMonth: true }))
}

/**
 * Every day of the local month containing `now`, as whole weeks.
 *
 * Whole weeks because a month grid with ragged rows is a grid whose columns stop
 * meaning a weekday, which is the one thing the layout is for. The cells outside
 * the month are flagged rather than omitted, so the grid can dim them and the
 * screen can still render their events — a class on the 1st of the next month is
 * visible in the trailing cells rather than nowhere.
 *
 * The month length is taken from the calendar rather than from a table: the first
 * day of the next month minus one day is the last day of this one, which is right
 * for a leap year without a leap-year rule here.
 */
export function localMonth(now: Date, zone: string): Day[] {
  const today = localDateKey(now, zone)
  const first = `${today.slice(0, 7)}-01`
  const nextMonth = addDays(first, 32).slice(0, 7)
  const last = addDays(`${nextMonth}-01`, -1)
  const length = Number(last.slice(8, 10))

  // Back to the Monday on or before the 1st, then forward in whole weeks.
  const back = (weekdayOf(first) + 6) % WEEK_LENGTH
  const start = addDays(first, -back)
  const cells = Math.ceil((back + length) / WEEK_LENGTH) * WEEK_LENGTH
  const month = first.slice(0, 7)

  return range(start, cells).map((key) => ({ key, inMonth: key.slice(0, 7) === month }))
}

/**
 * Whether `ends_at` falls exactly on a local midnight.
 *
 * One millisecond earlier is the test, and it is exact: the local day changes
 * across a boundary and nowhere else. This is `day.ts`'s rule, restated here
 * because the grid's placement is its own function and a shared helper would have
 * to be moved out of a file whose module comment owns it.
 */
function endsAtLocalMidnight(endsAt: string, timeZone: string): boolean {
  const at = Date.parse(endsAt)
  return localDateKey(new Date(at), timeZone) !== localDateKey(new Date(at - 1), timeZone)
}

/**
 * Whether an event overlaps the local day `key`.
 *
 * The end is the **exclusive** edge, which is the server's own overlap rule
 * (`starts_at < to and ends_at > from`): an event that ends exactly when a day
 * begins is not on that day.
 *
 * An event is placed on **every** local day it overlaps, not only its start day.
 * `GET /events` answers an overlap, so a conference that began last week arrives
 * in this week's window; placing it by its start day alone would drop it from the
 * grid with nothing on the page to say where it went.
 */
function overlapsDay(event: EventRow, key: string, timeZone: string): boolean {
  const startKey = localDateKey(new Date(event.starts_at), timeZone)
  const endKey = localDateKey(new Date(event.ends_at), timeZone)
  if (key < startKey || key > endKey) return false
  if (key === endKey) return !endsAtLocalMidnight(event.ends_at, timeZone)
  return true
}

/**
 * The days' events, one bucket per day, in the order the grid renders them.
 *
 * The bucket list is always exactly as long as the days it was given, in the same
 * order, including the days with nothing on them: a grid that skipped empty days
 * would put Tuesday's class under Monday's heading.
 *
 * Within a day the order is `starts_at` then `id`. Chronological, because a
 * timetable reads top to bottom, and `id` as the tiebreak so two events at the
 * same minute do not swap places between renders — a grid that reorders on every
 * pull is one the student cannot scan.
 */
export function placeEvents(events: EventRow[], days: Day[], zone: string): DayBucket[] {
  return days.map((day) => ({
    day,
    events: events
      .filter((event) => overlapsDay(event, day.key, zone))
      .sort((left, right) => {
        const byStart = Date.parse(left.starts_at) - Date.parse(right.starts_at)
        return byStart !== 0 ? byStart : left.id.localeCompare(right.id)
      }),
  }))
}

/**
 * The local calendar date `key` as a heading: "Wednesday, 10 June".
 *
 * Midday rather than midnight, so the instant is never on the far side of a DST
 * boundary from the date it names.
 */
export function dayHeading(key: string, zone: string): string {
  return formatInZone(new Date(`${key}T12:00:00Z`), zone, {
    weekday: 'long',
    day: 'numeric',
    month: 'long',
  })
}

/**
 * A workspace's course hue, or null when the event belongs to no workspace.
 *
 * **No table stores a course colour.** The spec's six hues are a categorical
 * palette, and the mapping from a workspace to one is derived here from the id so
 * that it is the same hue on every device, in every render, with no column to
 * keep in step. A `color` column on `workspaces` would be a second source of truth
 * for a value nobody edits, and it would have to be synced.
 *
 * The hue is never the only signal: the marker is a 4px bar **plus** a tinted chip
 * **plus** the workspace's initial and its name, which is the spec's rule and the
 * reason a monochrome print of the grid still says which course is which.
 */
export function courseHue(workspaceId: string | null): CourseName | null {
  if (workspaceId === null || workspaceId === '') return null
  // FNV-1a over the id: a cheap, stable hash with no crypto dependency. All that
  // is asked of it is that it does not change between builds and that neighbouring
  // ids do not all land on the same hue, which the avalanche of the multiply by
  // 0x01000193 is enough for at this size.
  let hash = 0x811c9dc5
  for (let index = 0; index < workspaceId.length; index += 1) {
    hash ^= workspaceId.charCodeAt(index)
    hash = Math.imul(hash, 0x01000193) >>> 0
  }
  return COURSE_NAMES[hash % COURSE_NAMES.length] ?? 'indigo'
}
