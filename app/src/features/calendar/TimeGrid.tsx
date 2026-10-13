import { Fragment, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import type { EventRow } from '@/api/queries/events'
import { Icon } from '@/components/ui/Icon'
// The display window is the **planner's**, imported and not redeclared: one window
// for the whole product, or the calendar's "now" line and the proposal grid's
// "now" line would sit at different heights for the same instant, and a study
// block accepted from the proposal would move when it appeared here.
import {
  GRID_END_HOUR,
  GRID_MINUTES,
  GRID_START_HOUR,
  localMinutesOfDay,
} from '@/features/planner/plan'
import { formatInZone } from '@/lib/time'
import { courseVars } from '@/tokens/tokens'
import { courseHue, type DayBucket } from './calendar'

/**
 * The week: one row per day, seven rows, each event a block placed by its start
 * and sized by its duration.
 *
 * A **table**, because a grid of absolutely-positioned `<div>`s is a grid a
 * keyboard cannot enter and a screen reader cannot read (A244). One row per day
 * rather than seven columns, because a seven-column grid is exactly the shape
 * that refuses to shrink at 200% text on a phone: a row per day is the same
 * information, reflows to one line per event by construction, and is what the
 * accessibility gate asks to see.
 *
 * **The bar is not where the time is said.** A block's position is the
 * `margin-left` and its duration the `width` of a proportional bar, as percentages
 * of the shared 07:00–22:00 window — a percentage is honest at every text scale,
 * where a pixel height would have to be re-measured for each one. The bar is
 * `aria-hidden` and the block always carries its `08:00–10:00` as text, because a
 * screen reader cannot measure a percentage and the time string is the fact. An
 * event with no duration at all has no height to be placed at, so it lives in the
 * all-day row.
 *
 * **The course colour is never the only signal.** Each event carries the 4px
 * marker **and** a tinted chip holding the workspace's initial **and** the
 * workspace's name, which is the spec's rule: a monochrome print of this grid, or
 * a reader who cannot distinguish indigo from sky, still knows which course is
 * which. An event with no workspace has none of the three, because there is no
 * course to mark.
 *
 * An external event is read-only, and says so three ways: a lock icon, the word
 * "Read-only", and an `aria-label` that names it — a state carried by shape, by
 * text and by the accessibility tree, never by colour.
 */

/** The id of an event's control, so the detail can hand focus back to it. */
export function eventCellId(eventId: string): string {
  return `calendar-event-${eventId}`
}

/** `08:00`, or `08:00–10:00` when the row has a duration, in the user's zone. */
function timeLabel(event: EventRow, zone: string): string {
  const start = formatInZone(new Date(event.starts_at), zone, {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  })
  const same = Date.parse(event.starts_at) === Date.parse(event.ends_at)
  if (same) return start
  const end = formatInZone(new Date(event.ends_at), zone, {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  })
  return `${start}\u2013${end}`
}

/**
 * A point event: one with no duration at all, i.e. `ends_at === starts_at`.
 *
 * That is how `kind: 'deadline'` is stored, and it is the **only** honest
 * definition available: the `events` table has no `all_day` column
 * (`api/queries/events.ts:69`, A199), so an all-day flag would be a field this
 * grid invented and the server would never send.
 */
function isPointEvent(event: EventRow): boolean {
  return Date.parse(event.starts_at) === Date.parse(event.ends_at)
}

/**
 * The state word for an event, or null when it has none.
 *
 * Two real fields decide it. `source` first: a row that is not the student's own
 * (`'google'` or `'ics'`) is **imported** and read-only, whatever its `kind`. Then,
 * among the student's own rows, `kind` and `task_id`: a `study_block` carrying a
 * `task_id` is the planner's **suggestion** still standing as an event, and one
 * without is a block the student holds. A class, an exam or a deadline carries no
 * state word — the three below are the states the app can honestly name, and
 * inventing a fourth would be inventing data.
 *
 * The three keys are spelled out at their call sites rather than looked up from a
 * table, because `i18n.test.ts` finds a used key by the literal in the `t('…')`
 * call: a dynamic key would look like copy nothing renders.
 */
function stateWord(event: EventRow, t: (key: string) => string): string | null {
  if (event.source !== 'app') return t('calendar.state.imported')
  if (event.kind !== 'study_block') return null
  return event.task_id === null ? t('calendar.state.scheduled') : t('calendar.state.suggested')
}

export function TimeGrid({
  buckets,
  zone,
  todayKey,
  now,
  selected,
  onSelect,
  workspaceNames,
  courseColors,
  detail,
}: {
  buckets: DayBucket[]
  zone: string
  todayKey: string
  /**
   * The instant the view was opened, fixed by the caller. The current-time line is
   * a function of **this** and never of a fresh `new Date()`, so two renders of the
   * same grid cannot disagree about where "now" is.
   */
  now: Date
  selected: string | null
  onSelect: (eventId: string) => void
  /** Workspace id → name, for the course name beside the marker. */
  workspaceNames: Record<string, string>
  /** The server's course index per workspace id, so the hue matches the course's card. */
  courseColors?: ReadonlyMap<string, number>
  /** The open event's detail, rendered inside its own day's row. */
  detail: (event: EventRow) => ReactNode
}) {
  const { t } = useTranslation()
  const first = buckets[0]?.day.key
  const last = buckets[buckets.length - 1]?.day.key
  const caption =
    first === undefined || last === undefined
      ? t('calendar.title')
      : t('calendar.weekOf', {
          from: formatInZone(new Date(`${first}T12:00:00Z`), zone, {
            day: 'numeric',
            month: 'short',
          }),
          to: formatInZone(new Date(`${last}T12:00:00Z`), zone, {
            day: 'numeric',
            month: 'short',
          }),
        })

  const nowMin = localMinutesOfDay(now, zone)
  const nowInWindow = nowMin >= GRID_START_HOUR * 60 && nowMin <= GRID_END_HOUR * 60

  // Every event in the window with **no duration**, de-duplicated by id:
  // `placeEvents` places an event on every local day it overlaps, so a row built by
  // concatenating the buckets could repeat one.
  const allDay: EventRow[] = []
  for (const bucket of buckets) {
    for (const event of bucket.events) {
      if (!isPointEvent(event)) continue
      if (allDay.some((seen) => seen.id === event.id)) continue
      allDay.push(event)
    }
  }

  /** One event's control. The same button in the all-day row and in a day's list. */
  const eventButton = (event: EventRow) => {
    const hue = courseHue(event.workspace_id, courseColors)
    const course =
      event.workspace_id === null ? null : (workspaceNames[event.workspace_id] ?? null)
    const external = event.source !== 'app'
    const state = stateWord(event, t)
    const time = timeLabel(event, zone)
    return (
      <button
        id={eventCellId(event.id)}
        type="button"
        // The label repeats what is on screen for an external row, because the
        // read-only state must reach the accessibility tree as a word and not only
        // as an icon.
        aria-label={
          external ? t('calendar.event.readOnlyLabel', { title: event.title, time }) : undefined
        }
        aria-expanded={event.id === selected}
        onClick={() => {
          onSelect(event.id)
        }}
        className="flex min-h-11 w-full min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-field px-2 py-2 text-left hover:bg-surface-raised focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring"
        style={hue === null ? undefined : courseVars(hue)}
      >
        {hue === null ? null : (
          <span className="course-marker h-5 shrink-0" aria-hidden="true" />
        )}
        {hue === null ? null : (
          <span
            className="chip shrink-0"
            aria-hidden="true"
            style={{
              backgroundColor: 'var(--course-tint)',
              color: 'var(--course-marker)',
            }}
          >
            {(course ?? event.title).charAt(0)}
          </span>
        )}
        {/* `min-w-0 break-words` and **no** `shrink-0` on the text items below. A
            flex item with `shrink-0` contributes its full width to the container's
            min-content width, so a row of non-shrinkable text items cannot compress
            below their sum — and inside a table cell that sum becomes the column's
            minimum and pushes the whole page wider. Measured at 200% text on a 390px
            phone: the time label alone sat 6.1px past the right edge and the page
            scrolled sideways. The marker and the single-initial chip keep
            `shrink-0`, because 4px and one character are sizes that cannot be the
            cause of an overflow. */}
        <span className="meta min-w-0 break-words">{time}</span>
        <span className="min-w-0 break-words text-body text-text">{event.title}</span>
        {course === null ? null : (
          <span className="min-w-0 break-words text-small text-muted">{course}</span>
        )}
        {state === null ? null : (
          // A state, so a chip: the word is always present and the tone repeats it.
          // `min-w-0` plus `overflow-wrap: anywhere` for the same reason as the
          // "Today" chip — a pill that cannot shrink is a pill that bleeds out of
          // its cell at 200% text.
          <span className="chip min-w-0" style={{ overflowWrap: 'anywhere' }}>
            {state}
          </span>
        )}
        {external && (
          <span className="flex min-w-0 items-center gap-1 text-caption text-muted">
            <Icon name="lock" size={16} />
            <span className="min-w-0 break-words">{t('calendar.event.readOnly')}</span>
          </span>
        )}
      </button>
    )
  }

  return (
    // A scroll container, and the same trade Phase 6 accepted for the tab bar. A
    // two-column grid is a *table*, and a table's `min-content` width is not a
    // number CSS can be told to shrink below: at 200% text the day column's label
    // plus one event's unbreakable time string came to **361.2px** against **310px**
    // of available width (the container's own `px-5` is `1.25rem`, which doubles to
    // 80px total at a 32px root), so the page scrolled sideways — measured
    // `scrollWidth` **430** against `clientWidth` **390**, both themes, and nothing
    // at 100%. Wrapping the table in its own scroller is what makes the *box* fit
    // while the content keeps its legible width: the reader scrolls one row instead
    // of losing the right-hand edge of every row. `overscroll-x-contain` keeps that
    // scroll from chaining to the page, and at 100% nothing scrolls, so the visual
    // is unchanged.
    <div className="w-full min-w-0 overflow-x-auto overscroll-x-contain">
      {/* `table-fixed`, like the month grid: with the automatic algorithm a table's
          `min-content` width is the sum of its cells' unbreakable content, and that
          number propagates up the ancestor chain until something gives — which is
          what put the page 40px wider than the phone. With a fixed layout the table
          takes the width it is given and the columns take the widths declared in the
          first row, so the box can shrink to any phone while the text inside wraps
          (`min-w-0 break-words` on every text item). Measured before the change: the
          page was 430 against a 390 viewport at 200% text in both themes. */}
      <table className="w-full table-fixed border-collapse text-left">
        {/* The caption is what names the table to a screen reader; it is visually
            hidden because the heading above the grid already says the same week. */}
        <caption className="sr-only">{caption}</caption>
        <thead>
          <tr>
            {/* `w-1/3` and not a quarter: at 200% text the day column has to hold
                "Monday" at 28px, and a quarter of a 310px content box minus the
                cell's own padding is 54px. The arbitrary `overflow-wrap: anywhere`
                is the load-bearing part — `break-word` does not reduce an item's
                intrinsic width, so the word stayed 109px wide inside a 54px box and
                bled into the events column. `anywhere` is what lets it wrap. */}
            <th scope="col" className="label w-1/3 border-b border-border pb-2 text-left">
              {t('calendar.column.day')}
            </th>
            <th scope="col" className="label border-b border-border pb-2 text-left">
              {t('calendar.column.events')}
            </th>
          </tr>
        </thead>
        <tbody>
          {/* The all-day row: every event in the window with no duration, which is
              the honest reading of a schema with no `all_day` flag (A199). It is the
              first row so a reader meets it before the days, and it is empty rather
              than padded with a placeholder when there is nothing in it. */}
          <tr className="border-b border-border align-top">
            <th scope="row" className="min-w-0 py-3 pr-3 text-left font-normal">
              <span
                className="min-w-0 break-words text-body font-medium text-text"
                style={{ overflowWrap: 'anywhere' }}
              >
                {t('calendar.allDay')}
              </span>
            </th>
            <td className="min-w-0 py-3">
              {allDay.length === 0 ? null : (
                <ul className="flex flex-col gap-3">
                  {allDay.map((event) => (
                    <li key={event.id} className="flex min-w-0 flex-col gap-1">
                      {eventButton(event)}
                    </li>
                  ))}
                </ul>
              )}
            </td>
          </tr>
          {buckets.map((bucket) => {
            const open = bucket.events.find((event) => event.id === selected) ?? null
            const timed = bucket.events.filter((event) => !isPointEvent(event))
            return (
              <Fragment key={bucket.day.key}>
                <tr className="border-b border-border align-top">
                  <th scope="row" className="min-w-0 py-3 pr-3 text-left font-normal">
                    <span className="flex min-w-0 flex-col gap-1">
                      <span
                        className="min-w-0 break-words text-body font-medium text-text"
                        style={{ overflowWrap: 'anywhere' }}
                      >
                        {formatInZone(new Date(`${bucket.day.key}T12:00:00Z`), zone, {
                          weekday: 'long',
                        })}
                      </span>
                      <span className="meta">
                        {formatInZone(new Date(`${bucket.day.key}T12:00:00Z`), zone, {
                          day: 'numeric',
                          month: 'short',
                        })}
                      </span>
                      {bucket.day.key === todayKey && (
                        // The chip is the second thing that would not shrink: a pill
                        // with `0.6rem` of padding each side plus "Today" at 24px is
                        // **108.6px** measured, against a 79px day column at 200%
                        // text, and `min-width: auto` on a flex item means it kept
                        // that width and bled out of its cell. `min-w-0` is the
                        // permission to shrink; `overflow-wrap: anywhere` is what
                        // makes the word itself wrap rather than overflow.
                        <span
                          className="chip min-w-0 self-start"
                          style={{ overflowWrap: 'anywhere' }}
                        >
                          {t('calendar.today')}
                        </span>
                      )}
                    </span>
                  </th>
                  <td className="min-w-0 py-3">
                    <ul className="flex flex-col gap-3">
                      {timed.map((event) => {
                        const startMin = localMinutesOfDay(event.starts_at, zone)
                        const endMin = localMinutesOfDay(event.ends_at, zone)
                        const left = ((startMin - GRID_START_HOUR * 60) / GRID_MINUTES) * 100
                        const width = ((endMin - startMin) / GRID_MINUTES) * 100
                        return (
                          <li key={event.id} className="flex min-w-0 flex-col gap-1">
                            {eventButton(event)}
                            {/* The block's place in the window, as a fraction of it:
                                `Math.max(0, …)` keeps an event that starts before
                                07:00 from pulling its bar off the left edge, and the
                                2% floor keeps a 15-minute block visible (it is 1.6%
                                of the window). An event crossing local midnight has
                                a negative duration here, so it is drawn as the floor
                                rather than as a backwards bar — its time label is
                                where the fact lives. */}
                            <span
                              aria-hidden="true"
                              className="h-1 rounded-pill bg-primary"
                              style={{
                                marginLeft: `${String(Math.max(0, left))}%`,
                                width: `${String(Math.max(2, width))}%`,
                              }}
                            />
                          </li>
                        )
                      })}
                      {timed.length === 0 && (
                        <li className="text-small text-subtle">{t('calendar.day.empty')}</li>
                      )}
                      {/* The current-time line, on today's row only, positioned from
                          the instant the view was opened — never from a fresh
                          `new Date()`. The label is not `shrink-0`: it is text, and a
                          text item that cannot shrink contributes its full width to
                          the cell's min-content width, which is the overflow this
                          file exists to prevent. The hairline is `flex-1`, so the
                          label keeps its intrinsic width anyway. */}
                      {bucket.day.key === todayKey && nowInWindow ? (
                        <li className="flex min-w-0 items-center gap-2">
                          <span className="meta min-w-0 break-words">{t('calendar.now')}</span>
                          <span
                            aria-hidden="true"
                            className="h-px flex-1 bg-border-strong"
                            style={{
                              marginLeft: `${
                                String(((nowMin - GRID_START_HOUR * 60) / GRID_MINUTES) * 100)
                              }%`,
                            }}
                          />
                        </li>
                      ) : null}
                    </ul>
                  </td>
                </tr>
                {open === null ? null : (
                  <tr className="border-b border-border">
                    <td colSpan={2} className="min-w-0 py-3">
                      {detail(open)}
                    </td>
                  </tr>
                )}
              </Fragment>
            )
          })}
        </tbody>
      </table>
    </div>
  )
}
