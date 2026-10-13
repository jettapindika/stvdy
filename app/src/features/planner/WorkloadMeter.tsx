import { useTranslation } from 'react-i18next'

import { plannedMinutes, type Interval } from './plan'

/**
 * The workload meter: planned minutes today, against the student's daily limit.
 *
 * **Minutes, not blocks.** The spec's meter is "planned minutes today vs the user's
 * daily limit", and the two units are not interchangeable: four 50-minute blocks
 * and four 20-minute blocks are the same number of blocks and 240 versus 80
 * minutes of study. So the bar is a proportion of minutes, and the label says the
 * number.
 *
 * The limit is the student's own `max_study_min_per_day` (A256), which is why it
 * arrives as a prop rather than being recomputed here: it is stored once, on
 * `user_settings`, and a second copy would be a second answer to one question.
 *
 * **Over the limit is a sentence as well as a tone.** Gate 5(d): a state is never
 * carried by colour alone, so an over-full day carries a word.
 *
 * **A limit of zero renders no proportion at all, and the sentence says why.**
 * `max_study_min_per_day` is a real setting and `0` is a legal value meaning "no
 * study time budgeted"; `planned / 0` is `Infinity`, and a bar drawn at `Infinity%`
 * is a bar whose width is whatever the browser decides. The number is still shown,
 * because it is a real measurement of the day, and the absence of a ratio is the
 * honest reading of a ratio with no denominator.
 */
export function WorkloadMeter({
  blocks,
  dayKey,
  timeZone,
  limitMin,
}: {
  blocks: readonly Interval[]
  dayKey: string
  timeZone: string
  limitMin: number
}) {
  const { t } = useTranslation()
  const planned = plannedMinutes(blocks, dayKey, timeZone)
  const over = limitMin > 0 && planned > limitMin
  const filled = limitMin > 0 ? Math.min(100, Math.round((planned / limitMin) * 100)) : 0

  return (
    <div className="card flex flex-col gap-3 p-5">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <h3 className="min-w-0 break-words">{t('planner.workload.title')}</h3>
        <p className="meta">
          {t('planner.workload.ofLimit', { planned, limit: limitMin })}
        </p>
      </div>

      {/* The bar is decorative: the number above it is the fact, and a screen
          reader user gets the sentence below rather than a percentage. */}
      {limitMin > 0 ? (
        <div aria-hidden="true" className="h-2 w-full overflow-hidden rounded-pill bg-surface-raised">
          <div
            className={over ? 'h-full rounded-pill bg-danger' : 'h-full rounded-pill bg-primary'}
            style={{ width: `${String(filled)}%` }}
          />
        </div>
      ) : null}

      {over ? (
        <p className="note note-warning wrap-anywhere">{t('planner.workload.over')}</p>
      ) : (
        <p className="text-small text-muted">{t('planner.workload.under')}</p>
      )}
    </div>
  )
}
