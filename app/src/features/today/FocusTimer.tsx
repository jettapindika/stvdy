import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useSettings } from '@/api/queries/settings'
import { WidgetHeading } from '@/components/layout/Section'
import { useFocusTimer } from './useFocusTimer'

/**
 * The focus timer's controls.
 *
 * Four things this component is careful about, and each is a gate rather than a
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
 *
 * **The block length is the account's setting, not this component's idea of a
 * session.** `block_len_min` and `break_min` come from `GET /settings`, so the
 * line reads the preference the planner also plans against. A session the student
 * stops after three minutes still logs three: the setting is a plan, and the
 * readout is what happened.
 */
export function FocusTimer() {
  const { t } = useTranslation()
  const { state, start, stop, discard, saveStale } = useFocusTimer()
  const settings = useSettings()
  const [edited, setEdited] = useState('')

  const clock =
    state.kind === 'idle'
      ? '00:00'
      : `${String(Math.floor(state.elapsedSec / 60)).padStart(2, '0')}:${String(
          state.elapsedSec % 60,
        ).padStart(2, '0')}`

  // The tone repeats what the word already says, so the state reads correctly in
  // monochrome and never depends on colour (gate 5d).
  const stateTone =
    state.kind === 'running'
      ? 'chip chip-working'
      : state.kind === 'stale'
        ? 'chip chip-due'
        : 'chip'
  const stateWord =
    state.kind === 'running'
      ? t('today.timer.running')
      : state.kind === 'stale'
        ? t('today.timer.stale')
        : t('today.timer.stopped')

  const block = settings.data?.settings

  return (
    <div className="card flex flex-col gap-4 p-6">
      <WidgetHeading title={t('today.timer.title')} />

      {/* The state word sits with the readout rather than inside the heading's
          flex row: at a 32px root the pair measured **233px inside a 212px
          column**, and a state that is clipped is a state that is not read. In a
          wrapping row it moves to its own line instead. */}
      <div className="flex flex-wrap items-center gap-x-6 gap-y-2">
        <span className={stateTone}>{stateWord}</span>
        {/* The design's readout is 36/47.5 mono, and the scale's nearest step is
            `text-h1` at 32/42 — the size utilities are a closed set and a new one
            is a change to the frozen stylesheet, so the readout takes the closest
            step rather than an inline pixel value nothing else can share. */}
        <p className="font-mono text-h1 tabular-nums">{clock}</p>
        {/* The readout is the measurement; the block length is the plan it was
            started against. Two lines, because "25 min" beside a running clock
            would read as the time elapsed. */}
        {block !== undefined ? (
          <p className="meta">
            {t('today.timer.sessionLength', {
              focus: block.block_len_min,
              rest: block.break_min,
            })}
          </p>
        ) : null}
      </div>

      {state.kind === 'idle' ? (
        <button type="button" className="btn btn-secondary self-start" onClick={() => void start()}>
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
