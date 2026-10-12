import { useTranslation } from 'react-i18next'

import { formatInZone } from '@/lib/time'
import { dayHeading, type DayBucket } from './calendar'

/**
 * The month: seven columns, one row per week.
 *
 * A month is a seven-column grid and cannot honestly be anything else — the
 * columns are the weekdays and the rows are the weeks, which is what makes the
 * shape readable at a glance. What that costs is room, so the cells carry the
 * **least** this screen can say and still be true: the day's number, and how many
 * events are on it. The event titles are the week view's job, one tap away.
 *
 * **One interactive target per cell, and it is the cell.** The month cell is a
 * button whose accessible name names the day, the date and the events on it, and
 * whose box is the whole cell — at least 44px in both dimensions, which is the
 * phone target the gate measures. A button per event would be a 50px-wide target
 * holding a 12px label, which is the shape the gate exists to refuse.
 *
 * The count is a real number from the rows (gate 7): it is the length of that
 * day's bucket, not an estimate, and it is rendered as a digit rather than as
 * dots, because a row of dots is a signal that only means something to a reader
 * who can see the colour of each one.
 *
 * The today cell says "Today" in its accessible name and carries `aria-current`,
 * which is the non-colour half of the state; the word itself is drawn when the
 * column is wide enough for it (`sm`), because a 50px cell at 200% text cannot
 * hold it — and a cell that clipped the word would be worse than one that keeps it
 * in the accessibility tree.
 */
export function MonthGrid({
  buckets,
  zone,
  todayKey,
  month,
  onOpenWeek,
}: {
  buckets: DayBucket[]
  zone: string
  todayKey: string
  /** `YYYY-MM`, so a cell outside the month can be dimmed rather than hidden. */
  month: string
  /** A day was chosen: the caller shows that day's week. */
  onOpenWeek: (key: string) => void
}) {
  const { t } = useTranslation()
  const weeks: DayBucket[][] = []
  for (let index = 0; index < buckets.length; index += 7) {
    weeks.push(buckets.slice(index, index + 7))
  }

  return (
    <table className="w-full table-fixed border-collapse">
      <caption className="sr-only">{t('calendar.month.caption')}</caption>
      <thead>
        <tr>
          {weeks[0]?.map((bucket) => (
            <th key={bucket.day.key} scope="col" className="label pb-2 text-center">
              {formatInZone(new Date(`${bucket.day.key}T12:00:00Z`), zone, {
                weekday: 'narrow',
              })}
            </th>
          ))}
        </tr>
      </thead>
      <tbody>
        {weeks.map((week) => (
          <tr key={week[0]?.day.key}>
            {week.map((bucket) => {
              const inMonth = bucket.day.key.slice(0, 7) === month
              const isToday = bucket.day.key === todayKey
              const count = bucket.events.length
              const names = bucket.events.map((event) => event.title).join(', ')
              return (
                <td key={bucket.day.key} className="min-w-0 p-0.5 align-top">
                  <button
                    type="button"
                    // The name is the day, the date, and what is on it. A cell
                    // whose name was only its number would leave a screen-reader
                    // user with no way to reach an event from this view.
                    aria-label={
                      count === 0
                        ? isToday
                          ? t('calendar.day.cellTodayEmpty', {
                              day: dayHeading(bucket.day.key, zone),
                            })
                          : t('calendar.day.cellEmpty', {
                              day: dayHeading(bucket.day.key, zone),
                            })
                        : isToday
                          ? t('calendar.day.cellToday', {
                              day: dayHeading(bucket.day.key, zone),
                              count,
                              events: names,
                            })
                          : t('calendar.day.cell', {
                              day: dayHeading(bucket.day.key, zone),
                              count,
                              events: names,
                            })
                    }
                    aria-current={isToday ? 'date' : undefined}
                    onClick={() => {
                      onOpenWeek(bucket.day.key)
                    }}
                    className={`flex min-h-11 w-full min-w-0 flex-col items-center gap-0.5 rounded-field px-1 py-1 focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring ${
                      inMonth ? 'text-text' : 'text-subtle'
                    } ${isToday ? 'bg-primary-tint font-medium text-primary-text' : ''}`}
                  >
                    <span className="text-small">{Number(bucket.day.key.slice(8, 10))}</span>
                    {/* The count, or nothing at all. A zero would be a number
                        standing for an absence, which the grid already shows by
                        being empty. */}
                    {count === 0 ? null : <span className="meta">{count}</span>}
                    {isToday ? (
                      <span className="hidden text-caption sm:inline">{t('calendar.today')}</span>
                    ) : null}
                  </button>
                </td>
              )
            })}
          </tr>
        ))}
      </tbody>
    </table>
  )
}
