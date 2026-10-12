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
    <table className="w-full border-collapse text-left">
      {/* The caption is what names the table to a screen reader; it is visually
          hidden because the heading above the grid already says the same week. */}
      <caption className="sr-only">{caption}</caption>
      <thead>
        <tr>
          <th scope="col" className="label w-32 border-b border-border pb-2 text-left">
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
                  <span className="flex flex-col gap-1">
                    <span className="text-body font-medium text-text">
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
                      <span className="chip self-start">{t('calendar.today')}</span>
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
                              <span className="meta shrink-0">{time}</span>
                              <span className="min-w-0 break-words text-body text-text">
                                {event.title}
                              </span>
                              {course === null ? null : (
                                <span className="min-w-0 break-words text-small text-muted">
                                  {course}
                                </span>
                              )}
                              {external && (
                                <span className="flex shrink-0 items-center gap-1 text-caption text-muted">
                                  <Icon name="lock" size={16} />
                                  {t('calendar.event.readOnly')}
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
  )
}
