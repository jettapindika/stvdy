import { Fragment, type ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import type { EventRow } from '@/api/queries/events'
import { Icon } from '@/components/ui/Icon'
import { formatInZone } from '@/lib/time'
import { courseVars } from '@/tokens/tokens'
import { courseHue, type DayBucket } from './calendar'

/**
 * The week: one row per day, seven rows.
 *
 * A **table**, because a grid of absolutely-positioned `<div>`s is a grid a
 * keyboard cannot enter and a screen reader cannot read (A244). One row per day
 * rather than seven columns, because a seven-column grid is exactly the shape
 * that refuses to shrink at 200% text on a phone: a row per day is the same
 * information, reflows to one line per event by construction, and is what the
 * accessibility gate asks to see.
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

export function WeekGrid({
  buckets,
  zone,
  todayKey,
  selected,
  onSelect,
  workspaceNames,
  detail,
}: {
  buckets: DayBucket[]
  zone: string
  todayKey: string
  selected: string | null
  onSelect: (eventId: string) => void
  /** Workspace id → name, for the course name beside the marker. */
  workspaceNames: Record<string, string>
  /** The open event's detail, rendered inside its own day's row. */
  detail?: (event: EventRow) => ReactNode
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
        {buckets.map((bucket) => {
          const open = bucket.events.find((event) => event.id === selected) ?? null
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
                  {bucket.events.length === 0 ? (
                    <p className="text-small text-subtle">{t('calendar.day.empty')}</p>
                  ) : (
                    <ul className="flex flex-col gap-2">
                      {bucket.events.map((event) => {
                        const hue = courseHue(event.workspace_id)
                        const course =
                          event.workspace_id === null
                            ? null
                            : (workspaceNames[event.workspace_id] ?? null)
                        const external = event.source !== 'app'
                        const time = timeLabel(event, zone)
                        return (
                          <li key={event.id} className="flex min-w-0">
                            <button
                              id={eventCellId(event.id)}
                              type="button"
                              // The label repeats what is on screen for an
                              // external row, because the read-only state must
                              // reach the accessibility tree as a word and not
                              // only as an icon.
                              aria-label={
                                external
                                  ? t('calendar.event.readOnlyLabel', {
                                      title: event.title,
                                      time,
                                    })
                                  : undefined
                              }
                              aria-expanded={event.id === selected}
                              onClick={() => {
                                onSelect(event.id)
                              }}
                              className="flex min-h-11 w-full min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-field px-2 py-2 text-left hover:bg-surface-raised focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring"
                              style={hue === null ? undefined : courseVars(hue)}
                            >
                              {hue === null ? null : (
                                <span
                                  className="course-marker h-5 shrink-0"
                                  aria-hidden="true"
                                />
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
                              {/* `min-w-0 break-words` and **no** `shrink-0` on the
                                  three text items below. A flex item with
                                  `shrink-0` contributes its full width to the
                                  container's min-content width, so a row of
                                  non-shrinkable text items cannot compress below
                                  their sum — and inside a table cell that sum
                                  becomes the column's minimum and pushes the whole
                                  page wider. Measured at 200% text on a 390px
                                  phone: the time label alone sat 6.1px past the
                                  right edge and the page scrolled sideways. The
                                  marker and the single-initial chip keep
                                  `shrink-0`, because 4px and one character are
                                  sizes that cannot be the cause of an overflow. */}
                              <span className="meta min-w-0 break-words">{time}</span>
                              <span className="min-w-0 break-words text-body text-text">
                                {event.title}
                              </span>
                              {course === null ? null : (
                                <span className="min-w-0 break-words text-small text-muted">
                                  {course}
                                </span>
                              )}
                              {external && (
                                <span className="flex min-w-0 items-center gap-1 text-caption text-muted">
                                  <Icon name="lock" size={16} />
                                  <span className="min-w-0 break-words">
                                    {t('calendar.event.readOnly')}
                                  </span>
                                </span>
                              )}
                            </button>
                          </li>
                        )
                      })}
                    </ul>
                  )}
                </td>
              </tr>
              {open === null || detail === undefined ? null : (
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
