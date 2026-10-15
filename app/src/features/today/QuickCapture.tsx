import { useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'

import { asApiError, type ApiError } from '@/api/errors'
import { quickCapture, saveTask, type QuickCaptureDraft, type TaskInput } from '@/api/queries/tasks'
import { useUsage } from '@/api/queries/usage'
import { WidgetHeading } from '@/components/layout/Section'
import { Icon } from '@/components/ui/Icon'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { TASK_CREDITS, planForFeature } from '@/features/billing/plans'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'

/**
 * Quick capture: one line in, a draft the student confirms.
 *
 * **Nothing is saved until Confirm.** The route writes no task (A193) and this
 * component writes none either: the draft is local state, and the only call to a
 * writer is behind the Confirm button. That is the mechanical defence against the
 * failure this feature exists to avoid — a task the student did not create, which
 * is invisible until they find it on their list.
 *
 * **The cost is on the button, and it is the matrix's number.** `TASK_CREDITS`
 * is read from `config/plans.toml` at build time (A190: quick capture is priced as
 * `task_breakdown`, because the matrix has no key of its own), so the price shown
 * is the price the server charges. A literal here would be a price the student was
 * quoted and not charged.
 *
 * **The field says what it is.** The composer hint sits under it, because the text
 * is free-form input that reaches a model.
 *
 * **A draft is labelled a draft.** `honesty.suggested` names it and
 * `honesty.nothingAddedYet` says what has not happened, and the model's own
 * warnings — a date it could not read, an estimate it had to clamp (A194) — are
 * shown beside the field they concern rather than logged. The student is the one
 * who can fix them.
 *
 * **A plan that does not grant it shows the locked state**, not a button that will
 * 403: the feature's answer comes from the matrix, and the plan named as having it
 * is derived from the same file rather than written into this component.
 *
 * **The due line is a date in the student's zone, not an instant.** The draft
 * carries a UTC instant, and printing it raw would name the wrong day: a task due
 * 06:00 on the 16th in Jakarta is `23:00Z` on the 15th. The zone comes from the
 * session, the same source `TodayScreen` uses for every other date on this screen
 * — a second source would be a second answer to "which day is it".
 *
 * **A draft can be dismissed from the keyboard.** Escape closes it and puts the
 * caret back in the composer: a draft is a proposal, and a keyboard user has to be
 * able to refuse one without a mouse. The listener is on `window` (the shape
 * `QuizScreen` and `StudyScreen` use for their global keys) because the card is a
 * transient overlay of the composer and focus can be anywhere when Escape arrives.
 */
export function QuickCapture() {
  const { t } = useTranslation()
  const usage = useUsage()
  const session = useSession()
  const timeZone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE
  const inputRef = useRef<HTMLInputElement>(null)
  const [text, setText] = useState('')
  const [draft, setDraft] = useState<QuickCaptureDraft | null>(null)
  const [editing, setEditing] = useState(false)
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<ApiError | null>(null)

  // Escape refuses the draft and hands the caret back to the composer. The
  // listener lives on `window` rather than on the card, because focus can be
  // anywhere when Escape is pressed — on the field, on one of the card's buttons,
  // or on nothing after a click on the page — and a handler on the card would miss
  // all but the first. It exists only while there is a draft to close, which is
  // the same shape the app's other global keys use (`QuizScreen`, `StudyScreen`).
  // Without the focus move, the caret lands on `document.body` and the next Tab
  // starts from the top of the document.
  useEffect(() => {
    if (draft === null) return
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== 'Escape') return
      setDraft(null)
      setEditing(false)
      inputRef.current?.focus()
    }
    window.addEventListener('keydown', onKeyDown)
    return () => {
      window.removeEventListener('keydown', onKeyDown)
    }
  }, [draft])

  const granted = usage.data?.features.quick_capture
  const cost = TASK_CREDITS.breakdown
  const minPlan = planForFeature('quick_capture')

  // Until the plan is known, nothing is offered. Offering the field and then
  // locking it would be a control that changed its mind.
  if (usage.data !== undefined && granted !== true) {
    if (minPlan === null) return null
    return (
      <LockedFeature
        whatKey="today.capture.title"
        minPlan={minPlan}
        reasonKey="today.capture.lockedReason"
      />
    )
  }

  const run = async () => {
    setBusy(true)
    setError(null)
    try {
      const result = await quickCapture(text.trim())
      setDraft(result)
      setEditing(false)
    } catch (failure) {
      setError(asApiError(failure))
    } finally {
      setBusy(false)
    }
  }

  const confirm = async () => {
    if (draft === null) return
    setBusy(true)
    setError(null)
    try {
      const input: TaskInput = {
        // The id is minted here, at the moment of the student's confirm, and not
        // when the draft arrived: a draft is a proposal, and an id minted for a
        // proposal that is then dismissed would be an identity for a row that
        // never existed.
        id: crypto.randomUUID(),
        workspace_id: null,
        parent_id: null,
        title: draft.title,
        notes: draft.notes,
        due_at: draft.due_at,
        estimate_min: draft.estimate_min,
        priority: draft.priority,
        status: 'todo',
        position: 0,
      }
      await saveTask(input, true)
      setDraft(null)
      setText('')
      setEditing(false)
    } catch (failure) {
      setError(asApiError(failure))
    } finally {
      setBusy(false)
    }
  }

  const update = (patch: Partial<QuickCaptureDraft>) => {
    setDraft((current) => (current === null ? current : { ...current, ...patch }))
  }

  return (
    <div className="card flex flex-col gap-3 p-5">
      <WidgetHeading title={t('today.capture.title')} />
      <label className="label" htmlFor="quick-capture-text">
        {t('today.capture.label')}
      </label>
      <input
        id="quick-capture-text"
        ref={inputRef}
        className="field"
        type="text"
        value={text}
        placeholder={t('today.capture.placeholder')}
        aria-describedby="quick-capture-hint"
        onChange={(event) => setText(event.target.value)}
      />
      {/* The hint under the field, because this text reaches a model and the
          draft is not saved until the student confirms it. It is this card's own
          line rather than the chat composer's: a captured task has no citations
          to check, and the frame's "Add a task now" would promise the one thing
          this card deliberately does not do (A193). */}
      <p id="quick-capture-hint" className="text-caption text-muted">
        {t('today.capture.hint')}
      </p>
      {/* The action is secondary rather than primary: this screen's one primary
          action is the review, and a second indigo fill would make two things
          look like the next step. */}
      <button
        type="button"
        className="btn btn-secondary w-full"
        disabled={busy || text.trim().length === 0}
        onClick={() => void run()}
      >
        <Icon name="sparkles" className="shrink-0" />
        {cost === null ? t('today.capture.run') : t('today.capture.runWithCost', { count: cost })}
      </button>

      {error !== null ? (
        <ApiErrorNotice error={error} noun={t('today.capture.noun')} action={t('actions.retry')} />
      ) : null}

      {draft !== null ? (
        <div className="raised flex flex-col gap-3 p-3">
          <p className="label">{t('honesty.suggested', { what: t('today.capture.draft') })}</p>
          <p className="text-small text-muted">{t('honesty.nothingAddedYet')}</p>

          {editing ? (
            <div className="flex flex-col gap-2">
              <label className="label" htmlFor="draft-title">
                {t('today.capture.fieldTitle')}
              </label>
              <input
                id="draft-title"
                className="field"
                value={draft.title}
                onChange={(event) => update({ title: event.target.value })}
              />
              <label className="label" htmlFor="draft-estimate">
                {t('today.capture.fieldEstimate')}
              </label>
              <input
                id="draft-estimate"
                className="field max-w-32"
                type="number"
                inputMode="numeric"
                min={1}
                value={draft.estimate_min ?? ''}
                onChange={(event) =>
                  update({
                    estimate_min: event.target.value === '' ? null : Number(event.target.value),
                  })
                }
              />
            </div>
          ) : (
            <ul className="flex flex-col gap-1">
              {/* `wrap-anywhere` on every string in this card, because each one is
                  text this component did not author — the model's title, a warning
                  it wrote, and a number it chose. Measured in Chromium at 390px with
                  the root font at 32px: a 55-character unbreakable title took
                  `document.scrollingElement.scrollWidth` to **1051** against a 390
                  client width, and `min-w-0 break-words` — the pair this app uses
                  where a *row* flex item has to shrink — left it at 1051, because
                  `overflow-wrap: break-word` does not lower a box's min-content
                  size. `overflow-wrap: anywhere` does, and brings it to 390. */}
              <li className="wrap-anywhere text-body">{draft.title}</li>
              {draft.due_at !== null ? (
                <li className="wrap-anywhere meta">
                  {/* The student's date, through the same helper and the same zone
                      the rest of this screen uses. The raw instant is UTC, which
                      names the wrong day for a deadline before 07:00 in Jakarta. */}
                  {t('today.capture.due', {
                    when: formatInZone(new Date(draft.due_at), timeZone, { dateStyle: 'medium' }),
                  })}
                </li>
              ) : null}
              {draft.estimate_min !== null ? (
                <li className="wrap-anywhere meta">
                  {t('today.capture.estimate', { count: draft.estimate_min })}
                </li>
              ) : null}
            </ul>
          )}

          {/* The model's warnings, as text beside the field they concern (A194).
              `wrap-anywhere` for the same reason as the list above: the detail is
              the model's own string, and a long one measured 1160px of page
              overflow at 390px with the root font at 32px. */}
          {draft.warnings.length > 0 ? (
            <ul className="flex flex-col gap-1">
              {draft.warnings.map((warning) => (
                <li key={warning} className="wrap-anywhere note note-warning">
                  {t('today.capture.warning', { detail: warning })}
                </li>
              ))}
            </ul>
          ) : null}

          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-primary"
              disabled={busy}
              onClick={() => void confirm()}
            >
              {t('actions.confirm')}
            </button>
            <button
              type="button"
              className="btn btn-secondary"
              onClick={() => setEditing((current) => !current)}
            >
              {t('actions.edit')}
            </button>
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => {
                setDraft(null)
                setEditing(false)
              }}
            >
              {t('actions.dismiss')}
            </button>
          </div>
        </div>
      ) : null}
    </div>
  )
}
