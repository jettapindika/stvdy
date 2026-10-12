import { localDateKey } from '@/lib/time'

/**
 * The planner's arithmetic, as pure functions.
 *
 * **`now` and the zone are parameters, and nothing here reads a clock.** That is
 * the same discipline `crates/core/src/planner/` follows: `PlannerInput.now` is a
 * field rather than a `Utc::now()` call, because a clock read inside the algorithm
 * is the one change no behavioural test has to notice. The week grid's "today",
 * the "days until the exam" figure and every block's position depend on the
 * student's *local* date, so the module takes the instant and the IANA zone and
 * computes from those — which is what makes a DST day testable and what stops a
 * device set to UTC from showing a Jakarta student's blocks on the wrong day.
 *
 * Two consequences of that rule are load-bearing here:
 *
 * - **A day boundary is the zone's day, not UTC's.** An exam at 06:00 Jakarta on
 *   the 16th is `23:00Z` on the **15th**; a UTC-based implementation puts it on
 *   the 16th and the student's "days until the exam" is off by one. Every date
 *   key in this module comes from `localDateKey`, never from `toISOString()`.
 * - **A pinned block has no subject of its own.** Contract § 6's `Interval`
 *   carries no goal attribution, so a pinned block comes back from the server
 *   with `goal_id: Uuid::nil()` and the topic `"Pinned"`. Printing that goal id
 *   would put a raw UUID on screen as if it were a subject; the preview labels it
 *   with a word instead, which is what {@link blockSubject} exists to decide.
 *
 * Nothing here talks to the network and nothing here renders. `PlannerScreen`
 * owns the requests and the copy; this module owns the rules.
 */

/** The topic a pinned block carries out of the pure planner (A265). */
export const PINNED_TOPIC = 'Pinned'

/** The nil UUID, which is what a pinned block's `goal_id` is. */
export const NIL_GOAL_ID = '00000000-0000-0000-0000-000000000000'

/** The spec's grid: 07:00 to 22:00. */
export const GRID_START_HOUR = 7
export const GRID_END_HOUR = 22

/** The grid's span in minutes, so a position is a fraction of one number. */
export const GRID_MINUTES = (GRID_END_HOUR - GRID_START_HOUR) * 60

/** The study window's own bounds, as `user_settings` stores them (A256). */
export interface StudySettings {
  start_hour: number
  end_hour: number
  max_study_min_per_day: number
}

/** A half-open instant range `[starts_at, ends_at)`, as ISO strings. */
export interface Interval {
  starts_at: string
  ends_at: string
}

/** One block, as `GET /plans/:id` serializes it (`ProposedBlock`). */
export interface ProposedBlock {
  starts_at: string
  ends_at: string
  goal_id: string
  topic: string
  /** `"NewMaterial"` or `"Review"` — the core enum's own spelling. */
  kind: string
  /** True for a block that came in through the planner's pinned set. */
  pinned: boolean
}

/** One day's load, as the planner computed it. */
export interface DayLoad {
  date: string
  planned_min: number
  capacity_min: number
}

/**
 * The two warnings the planner can raise (A262 — the enum is closed, and a third
 * variant is a contract change).
 */
export type PlanWarning =
  | { Overloaded: { days: string[] } }
  | { PastDeadline: { goal_id: string } }

/** The proposal itself, as the `proposal` column holds it. */
export interface Proposal {
  blocks: ProposedBlock[]
  day_load: DayLoad[]
  deficit_min: number
  warnings: PlanWarning[]
}

/** A block's place in the grid, in the student's zone. */
export interface GridBlock {
  block: ProposedBlock
  /** Local minutes from midnight, so the grid's position is arithmetic. */
  start_min: number
  end_min: number
  /** False when the block falls outside 07:00–22:00 and is clamped to the edge. */
  in_window: boolean
}

/** One row of the week grid. */
export interface GridDay {
  date: string
  blocks: GridBlock[]
}

/**
 * Whether a block is one the planner pinned — either because the server said so,
 * or because it is the nil-goal / `"Pinned"` pair a pinned block round-trips as.
 *
 * Both spellings mean the same thing, and a replan keeps such a block exactly
 * (contract § 6 step 7), so the preview must say so rather than showing it as
 * new material the student could still lose.
 */
export function isPinned(block: ProposedBlock): boolean {
  return block.pinned || isKeptBlock(block)
}

/**
 * Whether the block is the server's pinned round trip: no goal of its own and the
 * topic `"Pinned"`.
 *
 * This is `crates/server/src/planner/apply.rs`'s own predicate, restated here
 * because the *client* is what must not print the nil id — the server resolves it
 * back to the committed row by its exact range, so the client's job is to echo it
 * unchanged and to label it honestly.
 */
export function isKeptBlock(block: ProposedBlock): boolean {
  return block.goal_id === NIL_GOAL_ID && block.topic === PINNED_TOPIC
}

/**
 * What the preview prints as a block's subject.
 *
 * A discriminated union rather than a string, because the two cases are not the
 * same kind of thing: a goal block has a subject the student chose, and a kept
 * block has none — it is a session already in their calendar. Returning the empty
 * string for the second case would let a caller render a blank label and call it
 * done; this shape makes the caller choose the word.
 */
export type BlockSubject = { kind: 'kept' } | { kind: 'goal'; text: string }

/** The subject a block renders under, given a way to name a goal id. */
export function blockSubject(
  block: ProposedBlock,
  titleOf: (goalId: string) => string | undefined,
): BlockSubject {
  if (isKeptBlock(block)) return { kind: 'kept' }
  return { kind: 'goal', text: titleOf(block.goal_id) ?? block.topic }
}

/**
 * Whether a block is a review-reserved one (A269).
 *
 * The server writes it with a title that says "Review", and the balancer may not
 * move it (A268) — so the preview shows the word and the student sees why the
 * block is where it is.
 */
export function isReviewBlock(block: ProposedBlock): boolean {
  return block.kind.toLowerCase() === 'review'
}

/** The local date key of an instant, in the student's zone. */
function keyOf(instant: string | Date, timeZone: string): string {
  return localDateKey(typeof instant === 'string' ? new Date(instant) : instant, timeZone)
}

/**
 * Minutes from local midnight, in the student's zone.
 *
 * `hourCycle: 'h23'` because the default for `en-GB` renders midnight as `24`,
 * which would place a block that starts at 00:00 at the end of the grid.
 */
export function localMinutesOfDay(instant: string | Date, timeZone: string): number {
  const parts = new Intl.DateTimeFormat('en-GB', {
    timeZone,
    hourCycle: 'h23',
    hour: '2-digit',
    minute: '2-digit',
  }).formatToParts(typeof instant === 'string' ? new Date(instant) : instant)

  const hour = Number(parts.find((part) => part.type === 'hour')?.value ?? '0')
  const minute = Number(parts.find((part) => part.type === 'minute')?.value ?? '0')
  return hour * 60 + minute
}

/** `YYYY-MM-DD` shifted by whole days, through UTC midnights so no DST day is skipped. */
function addDays(key: string, days: number): string {
  const at = Date.parse(`${key}T00:00:00Z`) + days * 86_400_000
  return new Date(at).toISOString().slice(0, 10)
}

/** The day of the week a date key is, 0 = Sunday, without going through a local instant. */
function weekdayOf(key: string): number {
  return new Date(`${key}T00:00:00Z`).getUTCDay()
}

/**
 * The Monday of the local week containing `now`.
 *
 * Monday first, the same week boundary `features/calendar/calendar.ts` uses, so
 * the planner's grid and the calendar cannot name a different week for one day.
 */
export function weekStart(now: Date, timeZone: string): string {
  const today = localDateKey(now, timeZone)
  // getUTCDay: 0 = Sunday, so Sunday is six days past Monday rather than one
  // before it.
  const offset = (weekdayOf(today) + 6) % 7
  return addDays(today, -offset)
}

/** The seven date keys of the local week containing `now`, Monday first. */
export function weekDays(now: Date, timeZone: string): string[] {
  const start = weekStart(now, timeZone)
  return Array.from({ length: 7 }, (_, index) => addDays(start, index))
}

/** The local date key of `now` in the student's zone — the grid's "today". */
export function todayKey(now: Date, timeZone: string): string {
  return localDateKey(now, timeZone)
}

/**
 * Whole local days from `now` to `target`, negative once the date has passed.
 *
 * Both ends are reduced to a local date key and then to a UTC midnight, so the
 * difference is a count of *calendar days*: a DST day is 23 or 25 hours long and
 * a millisecond difference would round it to the wrong number of days.
 */
export function daysUntil(target: string, now: Date, timeZone: string): number {
  const from = Date.parse(`${localDateKey(now, timeZone)}T00:00:00Z`)
  const to = Date.parse(`${keyOf(target, timeZone)}T00:00:00Z`)
  return Math.round((to - from) / 86_400_000)
}

/** A block's place in the grid, in the student's zone. */
function placeBlock(block: ProposedBlock, timeZone: string): GridBlock {
  const start = localMinutesOfDay(block.starts_at, timeZone)
  const ends = localMinutesOfDay(block.ends_at, timeZone)
  const startMin = Math.max(GRID_START_HOUR * 60, start)
  // A block that ends before it starts crosses local midnight, so its end is the
  // grid's own end rather than a negative duration.
  const endMin = ends <= start ? GRID_END_HOUR * 60 : Math.min(GRID_END_HOUR * 60, ends)
  return {
    block,
    start_min: startMin,
    end_min: Math.max(startMin, endMin),
    in_window: start >= GRID_START_HOUR * 60 && endMin <= GRID_END_HOUR * 60 && endMin > startMin,
  }
}

/**
 * The week's blocks, one row per local day, each positioned by its local clock.
 *
 * A block is placed on the day it **starts** in the student's zone. That is the
 * same boundary the planner's own `DayLoad.date` uses, so the grid and the
 * workload meter cannot disagree about which day a block belongs to.
 */
export function gridDays(
  blocks: ProposedBlock[],
  days: string[],
  timeZone: string,
): GridDay[] {
  const byDay = new Map<string, GridBlock[]>(days.map((day) => [day, []]))
  for (const block of blocks) {
    const key = keyOf(block.starts_at, timeZone)
    byDay.get(key)?.push(placeBlock(block, timeZone))
  }
  for (const placed of byDay.values()) {
    placed.sort((left, right) => left.start_min - right.start_min || left.end_min - right.end_min)
  }
  return days.map((date) => ({ date, blocks: byDay.get(date) ?? [] }))
}

/**
 * The minutes of block time starting on the local day `dayKey`.
 *
 * **Minutes, not blocks.** The spec's meter is "planned minutes today vs the
 * user's daily limit", and a count of blocks cannot be compared with a limit in
 * minutes: four 50-minute blocks and four 20-minute blocks are the same number of
 * blocks and 240 versus 80 minutes of study.
 *
 * It takes the interval shape rather than a whole `ProposedBlock`, because the
 * meter's input is the student's *accepted* events — which are `EventRow`s, not
 * blocks — and the two must be measured by the same rule or the meter and the
 * preview would disagree about a day's load.
 */
export function plannedMinutes(
  blocks: readonly Interval[],
  dayKey: string,
  timeZone: string,
): number {
  return blocks.reduce((total, block) => {
    if (keyOf(block.starts_at, timeZone) !== dayKey) return total
    return total + blockMinutes(block)
  }, 0)
}

/** A block's length in whole minutes. */
export function blockMinutes(block: Interval): number {
  return Math.max(0, Math.round((Date.parse(block.ends_at) - Date.parse(block.starts_at)) / 60_000))
}

/** What the workload meter shows for one day. */
export interface Workload {
  planned_min: number
  limit_min: number
  /** True when the day is over the student's own daily maximum. */
  over: boolean
}

/**
 * The meter's numbers for one local day, from the blocks and the student's limit.
 *
 * The limit is the student's own `max_study_min_per_day` (A256), read from the
 * proposal's `day_load` when the server sent a row for that day and from the
 * settings otherwise: a day the planner never placed a block on has no
 * `day_load` row, and its capacity is still the limit.
 */
export function workloadFor(
  blocks: readonly Interval[],
  dayKey: string,
  timeZone: string,
  limitMin: number,
): Workload {
  const planned = plannedMinutes(blocks, dayKey, timeZone)
  return { planned_min: planned, limit_min: limitMin, over: planned > limitMin }
}

/** Why a drag was refused, in the words the preview renders. */
export type MoveRefusal = 'backwards' | 'busy' | 'window' | 'daily_max'

/** What the grid needs to judge a move: the busy time it was given and the window. */
export interface MoveContext {
  timeZone: string
  settings: StudySettings
  /** The busy intervals the proposal's grid already knows about (classes, exams). */
  busy: Interval[]
}

/**
 * Whether a block may sit at `[start, end)`, or why it may not.
 *
 * **This is the courtesy, not the control.** The server re-validates the merged
 * result on accept and refuses the whole batch rather than writing part of a plan
 * (A264), because a client's local rule is not a guarantee. The refusal is here
 * so a student dragging a block over a lecture is told immediately rather than
 * after a round trip that discards their whole plan.
 *
 * The three rules are the server's own three: inside the study window, not
 * overlapping busy time, and within the day's maximum.
 */
export function refusalFor(
  start: Date,
  end: Date,
  context: MoveContext,
  /** The minutes already planned on the target day, excluding the block being moved. */
  dayPlannedMin: number,
): MoveRefusal | null {
  if (end.getTime() <= start.getTime()) return 'backwards'

  const day = localDateKey(start, context.timeZone)
  if (localDateKey(end, context.timeZone) !== day) return 'window'

  const startMin = localMinutesOfDay(start, context.timeZone)
  const endMin = localMinutesOfDay(end, context.timeZone)
  const windowStart = context.settings.start_hour * 60
  const windowEnd = context.settings.end_hour * 60
  // The window's own end may be 24:00, which `localMinutesOfDay` renders as 0.
  if (startMin < windowStart) return 'window'
  if (endMin > windowEnd && endMin !== 0) return 'window'

  const from = start.getTime()
  const to = end.getTime()
  for (const interval of context.busy) {
    const busyFrom = Date.parse(interval.starts_at)
    const busyTo = Date.parse(interval.ends_at)
    if (from < busyTo && busyFrom < to) return 'busy'
  }

  const minutes = Math.round((to - from) / 60_000)
  if (dayPlannedMin + minutes > context.settings.max_study_min_per_day) return 'daily_max'

  return null
}

/** The outcome of a drag: the new block list, or the reason it was refused. */
export type MoveResult =
  | { ok: true; blocks: ProposedBlock[] }
  | { ok: false; reason: MoveRefusal }

/**
 * Moves one block by whole minutes, marking it as the student's own placement.
 *
 * A moved block becomes **pinned** (`pinned: true`), which is what the preview
 * says out loud: the student has decided where this session goes, and a replan
 * keeps it there (contract § 6 step 7). Without the flag a replan would be free to
 * move it back, and the student's adjustment would silently evaporate — the one
 * place the UI can create a state the planner then has to honour.
 *
 * The block keeps its own goal and topic. It is not the server's `nil`/`"Pinned"`
 * pair, which is only for a block that already exists as an event: this one has
 * not been accepted yet, so accept writes it at the dragged time.
 *
 * `dayPlannedMin` excludes the block being moved, so moving a block within its own
 * day is not judged against a total that already counts it.
 */
export function moveBlock(
  blocks: ProposedBlock[],
  index: number,
  deltaMinutes: number,
  context: MoveContext,
): MoveResult {
  const block = blocks[index]
  if (block === undefined) return { ok: false, reason: 'backwards' }

  const start = new Date(Date.parse(block.starts_at) + deltaMinutes * 60_000)
  const end = new Date(Date.parse(block.ends_at) + deltaMinutes * 60_000)

  const dayKey = localDateKey(start, context.timeZone)
  const others = blocks.filter((_, position) => position !== index)
  const dayPlanned = plannedMinutes(others, dayKey, context.timeZone)

  const refusal = refusalFor(start, end, context, dayPlanned)
  if (refusal !== null) return { ok: false, reason: refusal }

  const moved: ProposedBlock = {
    ...block,
    starts_at: start.toISOString(),
    ends_at: end.toISOString(),
    pinned: true,
  }
  return { ok: true, blocks: blocks.map((current, position) => (position === index ? moved : current)) }
}

/** One block of the accept body, in the shape `POST /plans/accept` reads. */
export interface AcceptBlock {
  starts_at: string
  ends_at: string
  /** `null` for a kept block: the server resolves it from the committed row. */
  goal_id: string | null
  topic: string
  kind: string
}

/**
 * The body `POST /plans/accept` takes, from the blocks the student is looking at.
 *
 * Two mappings matter and both are the server's contract rather than a choice
 * here:
 *
 * - **A kept block goes back as `goal_id: null` and the topic `"Pinned"`**, exactly
 *   as it arrived. The server resolves it to its committed event by the exact
 *   `(starts_at, ends_at)` range and refuses when that is ambiguous; inventing a
 *   goal id for it would be the client guessing at something it cannot know.
 * - **`pinned` is not sent.** `AcceptBlock` is `deny_unknown_fields`, so an extra
 *   field is a 400 for the whole batch — the drag's marker is the client's own
 *   bookkeeping and the server needs only the times.
 *
 * The kind is a string on the wire: `"review"` for a review-reserved block and
 * `"new_material"` for anything else, which is the pair the server's own test
 * accepts.
 */
export function acceptBody(blocks: ProposedBlock[]): { blocks: AcceptBlock[] } {
  return {
    blocks: blocks.map((block) => ({
      starts_at: block.starts_at,
      ends_at: block.ends_at,
      goal_id: isKeptBlock(block) ? null : block.goal_id,
      topic: block.topic,
      kind: isReviewBlock(block) ? 'review' : 'new_material',
    })),
  }
}

/** A warning as the copy layer needs it: a key and the values it interpolates. */
export interface WarningCopy {
  key: 'planner.overloaded' | 'planner.pastDeadline'
  count: number
  /** The goal's title, for `pastDeadline`. */
  goal?: string
}

/**
 * A warning as copy, never as a number the student has to interpret.
 *
 * Both warnings are sentences (gate 5(d): a state is never carried by colour
 * alone), and the overloaded one carries the *count of days* the planner could not
 * fit rather than a list of dates: the dates are on the grid, and a sentence
 * naming seven dates is not a sentence.
 */
export function warningCopy(
  warning: PlanWarning,
  titleOf: (goalId: string) => string | undefined,
): WarningCopy {
  if ('Overloaded' in warning) {
    return { key: 'planner.overloaded', count: warning.Overloaded.days.length }
  }
  const goal = titleOf(warning.PastDeadline.goal_id)
  return { key: 'planner.pastDeadline', count: 0, goal: goal ?? '' }
}

/**
 * Whether a proposal's day is over the student's limit, from the planner's own
 * `day_load` rather than from a second computation here.
 *
 * The server's `capacity_min` is the honest capacity — it is the window minus
 * busy time, which the client cannot recompute without the class calendar — so the
 * meter shows `planned_min` against it when the row exists.
 */
export function overloadedDays(dayLoad: DayLoad[]): string[] {
  return dayLoad.filter((day) => day.planned_min > day.capacity_min).map((day) => day.date)
}
