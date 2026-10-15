import { localDateKey } from '@/lib/time'
import { orderTasks, type TaskRow } from '@/features/tasks/order'
import type { EventRow } from '@/api/queries/events'
import type { SessionRow } from '@/api/queries/study'

/**
 * The Today view's rules, as pure functions.
 *
 * There is no `GET /today` route and there cannot be one: the roadmap's gate for
 * this phase is that the Today view works *offline*, and a server route cannot
 * (A205). So the buckets are computed here from rows the device already holds,
 * the same way `features/study/due.ts` computes due-ness — and for the same
 * reason: the rules that fail **silently** for an Indonesian student are the ones
 * worth unit-testing against a fixed clock rather than asserting on a screen.
 *
 * Three of those failures, all named because all three are invisible in a UTC-only
 * test and visible to most of this app's users:
 *
 *  - a task due at 07:00+07:00 is on today's list in Jakarta and on *yesterday's*
 *    in UTC;
 *  - a deadline at 23:30+07:00 is the 16th locally and the 16th in UTC, which is
 *    why both cases are asserted — the naive implementation gets the second one
 *    right and is still wrong;
 *  - a streak counted over UTC days breaks at a different point than the student's
 *    days do.
 *
 * Nothing here reads a clock: `now` is a parameter, which is what makes the whole
 * file testable and what stops a screen from showing a number the app did not
 * compute (gate 7a). No `Date.now()`, no fetch, no React.
 */

/** The three instants a streak is made of, as `GET /activity` returns them. */
export interface Activity {
  reviews: string[]
  attempts: string[]
  sessions: string[]
}

export interface TodayInput {
  now: Date
  /** The student's IANA zone. Never the machine's (A6). */
  timeZone: string
  tasks: TaskRow[]
  events: EventRow[]
  /**
   * The sessions the device knows about, with their durations.
   *
   * Separate from `activity` on purpose: `activity` is the three *instants* a
   * streak is made of (`GET /activity`), and minutes need durations, which that
   * route does not carry. Passing the session rows here is what keeps the two
   * questions — "did I study that day" and "how long today" — from being answered
   * by one array that cannot answer both.
   */
  sessions: SessionRow[]
  /** Due flashcards, as the deck read reports them. */
  dueCards: number
  activity: Activity
  /** Live workspaces, sources and decks, for the new-user checklist. */
  setup: { workspaces: number; sources: number; decks: number }
}

/** One row on today's timeline, tagged so the screen can render it as what it is. */
export type TodayEntry =
  | { kind: 'task'; id: string; at: string; task: TaskRow }
  | { kind: 'event'; id: string; at: string; event: EventRow }

/** One day of the streak strip: a local date, and whether anything happened on it. */
export interface RecentDay {
  /** The local calendar day, `YYYY-MM-DD`. */
  key: string
  /** Whether it holds any one of a review, an attempt or a session. */
  active: boolean
}

/**
 * How many days the streak strip draws. Seven — a full week, so the student
 * sees every day at a glance.
 */
export const RECENT_DAY_COUNT = 7

export interface TodayBuckets {
  /** Today's timeline: tasks due today and the events overlapping today, earliest first. */
  today: TodayEntry[]
  /** Tasks due before the local day started, and not done, in the one task order. */
  overdue: TaskRow[]
  /** Deadlines and exams in the next seven local days, starting with today. */
  thisWeek: EventRow[]
  /** Minutes logged in `study_sessions` today, in the student's zone. */
  minutesToday: number
  /**
   * Consecutive local days, ending today or yesterday, with a review, an attempt
   * or a session. A day counts if it has any one of the three.
   */
  streakDays: number
  /** The three steps the spec's new-user variant names, each with its own count. */
  setup: { workspaces: number; sources: number; decks: number }
  /** Echoed so one object holds every number the screen shows. */
  dueCards: number
  /**
   * The seven local days the week ahead covers, as calendar dates.
   *
   * The range is what the deadlines widget prints above its cards ("12–18
   * October"), and it is the same window `thisWeek` was filtered by — computed
   * once here rather than twice on the screen, where the two could drift.
   */
  week: { from: string; to: string }
  /**
   * The last five local days ending today, oldest first, each marked with whether
   * it holds a review, an attempt or a session. The streak card draws these.
   */
  recentDays: RecentDay[]
}

/**
 * The calendar date `days` after `key`.
 *
 * Pure integer arithmetic on the calendar, deliberately not `key + 24h`: adding a
 * day to a date is not adding a day to an instant, and the two differ on every
 * DST boundary. `Date.UTC` normalises the overflow, and the ISO prefix is the
 * calendar date it normalised to.
 */
function addDays(key: string, days: number): string {
  const [year, month, day] = key.split('-').map(Number)
  return new Date(Date.UTC(year ?? 0, (month ?? 1) - 1, (day ?? 1) + days))
    .toISOString()
    .slice(0, 10)
}

/** The local date key of an instant, as `YYYY-MM-DD` in the student's zone. */
function keyOf(instant: string, timeZone: string): string {
  return localDateKey(new Date(instant), timeZone)
}

/**
 * How many calendar days separate two local date keys, or null when `key` is
 * before `from` or past the end of the window.
 *
 * The relative label on a deadline ("Today", "In 2 days") is this number, and it
 * has to be a difference of *calendar dates* rather than of instants: a deadline
 * at 23:59 and one at 00:01 are a day apart although they are two minutes apart,
 * and a millisecond division would call them the same day.
 */
export function dayOffset(from: string, key: string, limit: number): number | null {
  for (let offset = 0; offset <= limit; offset += 1) {
    if (addDays(from, offset) === key) return offset
  }
  return null
}

/**
 * Whether `endsAt` falls exactly on a local midnight, so an event that ends when
 * the day begins is not counted as being *on* that day.
 *
 * One millisecond earlier is the test, and it is exact: the local day changes
 * across a boundary and nowhere else. A heuristic on the formatted time would have
 * to decide whether 00:00:30 is midnight.
 */
function endsAtLocalMidnight(endsAt: string, timeZone: string): boolean {
  const at = Date.parse(endsAt)
  return keyOf(endsAt, timeZone) !== localDateKey(new Date(at - 1), timeZone)
}

/** Whether an event overlaps the local day `dayKey`. */
function overlapsDay(event: EventRow, dayKey: string, timeZone: string): boolean {
  const startsKey = keyOf(event.starts_at, timeZone)
  if (startsKey > dayKey) return false
  const endsKey = keyOf(event.ends_at, timeZone)
  if (endsKey > dayKey) return true
  // The end is the exclusive edge: an event that ends exactly when the day begins
  // is not on this day. This is the case a naive `<=` gets wrong, and it is the
  // one the server's own overlap rule states as `starts_at < to and ends_at > from`.
  return endsKey === dayKey && !endsAtLocalMidnight(event.ends_at, timeZone)
}

/** A task that is still outstanding: not done and not tombstoned. */
function isOutstanding(task: TaskRow): boolean {
  return task.deleted_at == null && task.status !== 'done'
}

/**
 * Today's buckets, from the rows the device already holds.
 *
 * `overdue` and `today` are disjoint: a task due before the local day started is
 * overdue and is *not* also on today's timeline. Showing it twice is what a
 * "due today" filter written as a range produces, and the student sees the same
 * task in two lists with no way to tell which one is the real one.
 */
export function todayBuckets(input: TodayInput): TodayBuckets {
  const { now, timeZone, tasks, events } = input
  const todayKey = localDateKey(now, timeZone)

  const dueToday: TaskRow[] = []
  const overdue: TaskRow[] = []
  for (const task of tasks) {
    if (!isOutstanding(task) || task.due_at == null) continue
    const dueKey = keyOf(task.due_at, timeZone)
    if (dueKey === todayKey) dueToday.push(task)
    else if (dueKey < todayKey) overdue.push(task)
  }

  const todayEntries: TodayEntry[] = [
    ...orderTasks(dueToday).map((task) => ({
      kind: 'task' as const,
      id: task.id,
      at: task.due_at ?? '',
      task,
    })),
    ...events
      .filter((event) => overlapsDay(event, todayKey, timeZone))
      .map((event) => ({ kind: 'event' as const, id: event.id, at: event.starts_at, event })),
  ]
  // A timeline is chronological. The task order above answers "what should I do
  // first"; this answers "what happens next", and a timeline that put a 21:00
  // deadline above a 09:00 class because the deadline has a higher priority would
  // be a list pretending to be a day.
  todayEntries.sort((left, right) => {
    const byTime = Date.parse(left.at) - Date.parse(right.at)
    return byTime !== 0 ? byTime : left.id.localeCompare(right.id)
  })

  const weekEnd = addDays(todayKey, 6)
  const thisWeek = events
    .filter((event) => event.kind === 'deadline' || event.kind === 'exam')
    .filter((event) => {
      const startsKey = keyOf(event.starts_at, timeZone)
      return startsKey >= todayKey && startsKey <= weekEnd
    })
    .sort((left, right) => {
      const byTime = Date.parse(left.starts_at) - Date.parse(right.starts_at)
      return byTime !== 0 ? byTime : left.id.localeCompare(right.id)
    })

  const minutesToday = input.sessions
    .filter((session) => keyOf(session.started_at, timeZone) === todayKey)
    .reduce((total, session) => total + session.duration_min, 0)

  return {
    today: todayEntries,
    overdue: orderTasks(overdue),
    thisWeek,
    minutesToday,
    streakDays: streakDays(input.activity, todayKey, timeZone),
    setup: input.setup,
    dueCards: input.dueCards,
    week: { from: todayKey, to: weekEnd },
    recentDays: recentDays(input.activity, todayKey, timeZone),
  }
}

/**
 * The local days that hold a review, an attempt or a session — any one of the
 * three, which is the spec's rule.
 *
 * One function for both the streak count and the strip, because a strip built
 * from its own reading of the activity would eventually fill a day the streak
 * says is empty: two answers to one question, and the student is looking at both.
 */
function activeDays(activity: Activity, timeZone: string): Set<string> {
  const active = new Set<string>()
  for (const instant of [...activity.reviews, ...activity.attempts, ...activity.sessions]) {
    active.add(keyOf(instant, timeZone))
  }
  return active
}

/**
 * The last `RECENT_DAY_COUNT` local days, ending today, oldest first.
 */
function recentDays(activity: Activity, todayKey: string, timeZone: string): RecentDay[] {
  const active = activeDays(activity, timeZone)

  const days: RecentDay[] = []
  for (let back = RECENT_DAY_COUNT - 1; back >= 0; back -= 1) {
    const key = addDays(todayKey, -back)
    days.push({ key, active: active.has(key) })
  }
  return days
}

/**
 * The streak, over local days.
 *
 * The walk starts at today and, when today is empty, at yesterday: a student who
 * has not opened the app yet today still has the streak they built, and reporting
 * zero at 09:00 would be the app telling them they lost something they did not
 * lose. From there it stops at the first day with nothing, which is what makes it
 * a streak rather than a count of active days.
 */
function streakDays(activity: Activity, todayKey: string, timeZone: string): number {
  const active = activeDays(activity, timeZone)

  let day = todayKey
  if (!active.has(day)) {
    const yesterday = addDays(todayKey, -1)
    if (!active.has(yesterday)) return 0
    day = yesterday
  }

  let days = 0
  while (active.has(day)) {
    days += 1
    day = addDays(day, -1)
  }
  return days
}
