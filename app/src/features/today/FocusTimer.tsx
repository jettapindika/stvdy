import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useFocusTimer } from './useFocusTimer'

/**
 * The focus timer's controls.
 *
 * Three things this component is careful about, and each is a gate rather than a
 * preference.
 *
 * **The state is a word, never a colour.** "Running", "Stopped" and the stale
 * warning are text beside the clock, so the state survives monochrome and reaches
 * a screen reader (gate 5d).
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
 */
export function FocusTimer() {
  const { t } = useTranslation()
  const { state, start, stop, discard, saveStale } = useFocusTimer()
  const [edited, setEdited] = useState('')

  const clock =
    state.kind === 'idle'
      ? '00:00'
      : `${String(Math.floor(state.elapsedSec / 60)).padStart(2, '0')}:${String(
          state.elapsedSec % 60,
        ).padStart(2, '0')}`

  return (
    <div className="card flex flex-col gap-4 p-4">
      <div className="flex flex-wrap items-center justify-between gap-3">
        <p className="text-h1 tabular-nums">{clock}</p>
        {state.kind === 'running' ? (
          <span className="chip chip-working">{t('today.timer.running')}</span>
        ) : null}
        {state.kind === 'stale' ? (
          <span className="chip chip-due">{t('today.timer.stale')}</span>
        ) : null}
        {state.kind === 'idle' ? (
          <span className="chip">{t('today.timer.stopped')}</span>
        ) : null}
      </div>

      {state.kind === 'idle' ? (
        <button
          type="button"
          className="btn btn-primary self-start"
          onClick={() => void start()}
        >
          {t('today.timer.start')}
        </button>
      ) : null}

      {state.kind === 'running' ? (
        <button
          type="button"
          className="btn btn-secondary self-start"
          onClick={() => void stop()}
        >
          {t('today.timer.stopAndLog', { count: state.elapsedMin })}
        </button>
      ) : null}

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
            <button
              type="button"
              className="btn btn-primary"
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
