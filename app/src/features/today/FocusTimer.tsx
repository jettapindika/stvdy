import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useSettings } from '@/api/queries/settings'
import { WidgetHeading } from '@/components/layout/Section'
import { Icon } from '@/components/ui/Icon'
import { useFocusTimer } from './useFocusTimer'

/**
 * The focus timer's controls.
 *
 * Four things this component is careful about, and each is a gate rather than a
 * preference.
 *
 * **The state is a word, never a colour.** "Ready", "Running" and "Needs your
 * call" are text, so the state survives monochrome and reaches a screen reader
 * (gate 5d).
 *
 * **The number that gets logged is on the button.** "Stop and log 12 min" is the
 * same `elapsedMin` the hook writes, so a student is never surprised by what the
 * session records — and the rounding that turns twenty seconds into one minute is
 * visible before it happens rather than discovered in the streak (gate 7b).
 *
 * **A stale timer is a question, not a decision.** A timer past the four-hour
 * bound shows what was measured and offers Save (with the minutes editable,
 * because the student knows how much of that window was study) or Discard, which
 * writes nothing at all. It never logs itself (A207).
 *
 * **The block length is the account's setting, not this component's idea of a
 * session.** `block_len_min` and `break_min` come from `GET /settings`, so the
 * line reads the preference the planner also plans against. A session the student
 * stops after three minutes still logs three: the setting is a plan, and the
 * readout is what happened.
 *
 * **The frame's arrangement is a row**: the readout, the block it was started
 * against, and the one control at the right end. The state word rides in the
 * heading rather than beside the readout, because a card heading's row is where
 * the frame puts a status and the readout's row has no room for a second string
 * at 200% text.
 *
 * **The frame's "what you are working on" line is not built, and that is the
 * honesty rule rather than an omission.** The design names a study block there;
 * nothing in this app knows what the student intends to work on next. A timer
 * started from this screen carries no `taskId`, so the only source for that line
 * would be a guess about the next row on the timeline — and a label naming a
 * different task than the session logs is worse than a shorter card.
 */
export function FocusTimer() {
  const { t } = useTranslation()
  const { state, start, stop, discard, saveStale } = useFocusTimer()
  const settings = useSettings()
  const [edited, setEdited] = useState('')

  const block = settings.data?.settings

  // The readout is **elapsed time** and stays elapsed when nothing is running.
  // The Figma frame shows the block length at idle (`25:00`), but the number that
  // gets logged is the elapsed time — never the configured block length
  // (useFocusTimer.ts:26-33). Showing a countdown would display a number that
  // disagrees with what the button beside it says ("Stop and log 5 min" while the
  // readout reads 20:00). The plan is stated in the session-length line instead.
  const clock =
    state.kind === 'idle'
      ? '00:00'
      : `${String(Math.floor(state.elapsedSec / 60)).padStart(2, '0')}:${String(
          state.elapsedSec % 60,
        ).padStart(2, '0')}`

  const stateWord =
    state.kind === 'running'
      ? t('today.timer.running')
      : state.kind === 'stale'
        ? t('today.timer.stale')
        : t('today.timer.ready')

  return (
    <div className="card @container flex flex-col gap-4 p-6">
      <WidgetHeading
        title={t('today.timer.title')}
        action={<span className="eyebrow">{stateWord}</span>}
      />

      <div className="flex flex-wrap items-center gap-x-5 gap-y-3">
        <p
          className="font-mono leading-normal tabular-nums"
          style={{ fontSize: `min(var(--text-h1), ${90 / (0.6 * clock.length)}cqi)` }}
        >
          {clock}
        </p>

        {/* The block length is the plan the session runs against; mono caps
            matches the spec's metadata treatment and the Figma frame. Rendered
            as a plain auto-basis child so `flex-wrap` can move it to its own
            line at 200% text — `flex-1 min-w-0` would give it a hypothetical
            main size of 0 and squeeze it into leftover space instead. */}
        {block !== undefined ? (
          <p className="eyebrow">{t('today.timer.sessionLength', {
            focus: block.block_len_min,
            rest: block.break_min,
          })}</p>
        ) : null}

        {state.kind === 'idle' ? (
          <button type="button" className="btn btn-secondary ml-auto shrink-0" onClick={() => void start()}>
            <Icon name="play" size={16} />
            {t('today.timer.start')}
          </button>
        ) : null}

        {state.kind === 'running' ? (
          <button type="button" className="btn btn-secondary ml-auto shrink-0" onClick={() => void stop()}>
            {t('today.timer.stopAndLog', { count: state.elapsedMin })}
          </button>
        ) : null}
      </div>

      {state.kind === 'stale' ? (
        <div className="flex flex-col gap-3">
          {/* The reason, in words. A timer that quietly stopped counting would be
              a student finding out from their streak. */}
          <p className="note note-warning">{t('today.timer.staleReason')}</p>
          <div className="flex flex-col gap-1">
            <label className="label" htmlFor="focus-timer-minutes">
              {t('today.timer.minutesLabel')}
            </label>
            <input
              id="focus-timer-minutes"
              className="field max-w-32"
              type="number"
              inputMode="numeric"
              min={1}
              max={state.elapsedMin}
              value={edited === '' ? String(state.elapsedMin) : edited}
              onChange={(event) => setEdited(event.target.value)}
            />
          </div>
          <div className="flex flex-wrap gap-2">
            {/* Secondary, not primary: this screen's one primary action is the
                review, and a stale timer is a question rather than the page's
                main move. */}
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => void saveStale(Number(edited === '' ? state.elapsedMin : edited))}
            >
              {t('today.timer.save')}
            </button>
            <button type="button" className="btn btn-quiet" onClick={() => void discard()}>
              {t('today.timer.discard')}
            </button>
          </div>
        </div>
      ) : null}
    </div>
  )
}
