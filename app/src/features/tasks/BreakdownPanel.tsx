import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { asApiError, type ApiError } from '@/api/errors'
import {
  breakdownTask,
  saveTask,
  type BreakdownProposal,
  type TaskInput,
} from '@/api/queries/tasks'
import { useUsage } from '@/api/queries/usage'
import { Section } from '@/components/layout/Section'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice } from '@/components/ui/States'
import { Icon } from '@/components/ui/Icon'
import { TASK_CREDITS, planForFeature } from '@/features/billing/plans'
import { dueLabel } from './parts'
import { durationLabel } from './summary'

/**
 * The break-down proposal, as a proposal.
 *
 * **Nothing is written until Accept.** The route returns the proposal and saves
 * nothing (A195); Accept is N ordinary creates, one per kept subtask, each with
 * `parent_id` and its position in the list — which is what makes the order the
 * model wrote the order the student sees, even after a batch insert (A196).
 *
 * **The estimates are the student's after they touch them.** The model's number is
 * shown inside the `Suggested …` region and the total is recomputed here as the
 * sum of what is left, so a row the student unchecks or edits is reflected before
 * anything is stored. Dismiss writes nothing at all.
 *
 * **The cost is on the button**, from `TASK_CREDITS` rather than a literal (A190).
 *
 * **The frame draws this as a modal and it is built as a section.** A modal is a
 * focus trap, and this panel is one of several blocks on the planner's tasks view
 * — trapping focus inside it would hide the task it is breaking down, which is the
 * one thing the student needs to read while deciding. It renders where it was
 * opened, at the end of the panel, and the panel's own close control is the way
 * out. That is the same trade `CalendarScreen`'s event detail makes, and for the
 * same reason.
 *
 * **The due line is the task's real `due_at`, and the frame's "DUE FRI 16 OCT ·
 * 23:59" is not reproduced from the design's illustrative date.** It arrives as a
 * prop because this panel does not read the task: it was opened with an id, and
 * fetching the row again to render one line would be a second read of the same
 * list the parent already holds.
 */
export function BreakdownPanel({
  taskId,
  taskTitle,
  dueAt,
  timeZone,
  onClose,
}: {
  taskId: string
  /** The task's own title, for the panel's description. */
  taskTitle: string
  /** The task's `due_at`, or null. Rendered in the student's zone. */
  dueAt: string | null
  timeZone: string
  onClose: () => void
}) {
  const { t } = useTranslation()
  const usage = useUsage()
  const [proposal, setProposal] = useState<BreakdownProposal | null>(null)
  const [kept, setKept] = useState<Record<number, { checked: boolean; minutes: number }>>({})
  const [busy, setBusy] = useState(false)
  const [error, setError] = useState<ApiError | null>(null)

  const cost = TASK_CREDITS.breakdown
  const minPlan = planForFeature('task_ai_breakdown')

  const run = async () => {
    setBusy(true)
    setError(null)
    try {
      const result = await breakdownTask(taskId)
      setProposal(result)
      setKept(
        Object.fromEntries(
          result.subtasks.map((subtask, index) => [
            index,
            { checked: true, minutes: subtask.estimate_min },
          ]),
        ),
      )
    } catch (failure) {
      setError(asApiError(failure))
    } finally {
      setBusy(false)
    }
  }

  const accept = async () => {
    if (proposal === null) return
    setBusy(true)
    setError(null)
    try {
      // The position is the row's index in the proposal, which is the order the
      // model wrote (A196). It is taken from the list rather than from a counter,
      // so unchecking a row does not renumber the ones after it and produce an
      // order the student never saw.
      for (const [index, subtask] of proposal.subtasks.entries()) {
        const row = kept[index]
        if (row === undefined || !row.checked) continue
        const input: TaskInput = {
          id: crypto.randomUUID(),
          workspace_id: null,
          parent_id: taskId,
          title: subtask.title,
          notes: null,
          due_at: null,
          estimate_min: row.minutes,
          priority: 2,
          status: 'todo',
          position: index,
        }
        await saveTask(input, true)
      }
      onClose()
    } catch (failure) {
      setError(asApiError(failure))
    } finally {
      setBusy(false)
    }
  }

  const granted = usage.data?.features.task_ai_breakdown
  if (usage.data !== undefined && granted !== true && minPlan !== null) {
    return (
      <Section title={t('tasks.breakdown.title')}>
        <LockedFeature
          whatKey="tasks.breakdown.title"
          minPlan={minPlan}
          reasonKey="tasks.breakdown.lockedReason"
        />
      </Section>
    )
  }

  const total =
    proposal === null
      ? 0
      : proposal.subtasks.reduce(
          (sum, _subtask, index) =>
            kept[index]?.checked === true ? sum + (kept[index]?.minutes ?? 0) : sum,
          0,
        )
  const keptCount =
    proposal === null
      ? 0
      : proposal.subtasks.filter((_subtask, index) => kept[index]?.checked === true).length

  return (
    <Section title={t('tasks.breakdown.title')}>
      <div className="card flex flex-col gap-4 p-5">
        <div className="flex flex-wrap items-start justify-between gap-3">
          <div className="flex min-w-0 flex-col gap-1">
            <p className="meta min-w-0 break-words text-primary-text">
              {t('tasks.breakdown.forTask', { task: taskTitle })}
            </p>
            <p className="min-w-0 break-words text-small text-muted">
              {t('honesty.nothingAddedYet')}
            </p>
          </div>
          <button type="button" className="btn btn-quiet" onClick={onClose}>
            {t('actions.close')}
          </button>
        </div>

        <button
          type="button"
          className="btn btn-primary self-start"
          disabled={busy}
          onClick={() => void run()}
        >
          {cost === null
            ? t('tasks.breakdown.run')
            : t('tasks.breakdown.runWithCost', { count: cost })}
        </button>

        {error !== null ? (
          <ApiErrorNotice error={error} noun={t('tasks.noun')} action={t('actions.retry')} />
        ) : null}

        {proposal !== null ? (
          <div className="raised flex flex-col gap-4 border border-border p-4">
            <div className="flex flex-col gap-1">
              <p className="eyebrow-primary">
                {t('honesty.suggested', { what: t('tasks.breakdown.what') })}
              </p>
              <p className="min-w-0 break-words text-small text-muted">
                {t('tasks.breakdown.description', { task: taskTitle })}
              </p>
            </div>

            <ul className="flex flex-col gap-3">
              {proposal.subtasks.map((subtask, index) => (
                <li key={`${subtask.title}-${String(index)}`} className="flex min-w-0 flex-wrap items-center gap-3">
                  <input
                    id={`subtask-${String(index)}`}
                    type="checkbox"
                    checked={kept[index]?.checked ?? false}
                    onChange={(event) =>
                      setKept((current) => ({
                        ...current,
                        [index]: {
                          checked: event.target.checked,
                          minutes: current[index]?.minutes ?? subtask.estimate_min,
                        },
                      }))
                    }
                  />
                  <label htmlFor={`subtask-${String(index)}`} className="min-w-0 flex-1 break-words text-body">
                    {subtask.title}
                  </label>
                  <span className="flex min-w-0 items-center gap-2">
                    <label className="label" htmlFor={`subtask-minutes-${String(index)}`}>
                      {t('tasks.breakdown.minutes')}
                    </label>
                    <input
                      id={`subtask-minutes-${String(index)}`}
                      className="field max-w-24"
                      type="number"
                      inputMode="numeric"
                      min={1}
                      value={kept[index]?.minutes ?? subtask.estimate_min}
                      onChange={(event) =>
                        setKept((current) => ({
                          ...current,
                          [index]: {
                            checked: current[index]?.checked ?? true,
                            minutes: Number(event.target.value),
                          },
                        }))
                      }
                    />
                  </span>
                </li>
              ))}
            </ul>

            <p className="meta">{t('tasks.breakdown.helper')}</p>

            <div className="flex flex-wrap gap-4">
              {/* The total as the sum of what is left, recomputed here rather than
                  read from the model's `total_estimate_min`: the model's number
                  describes the list it proposed, and the student has changed it. */}
              <div className="flex min-w-0 flex-col">
                <span className="text-h4 min-w-0 break-words">
                  {t('tasks.breakdown.totalEstimate', { duration: durationLabel(t, total) })}
                </span>
                <span className="meta">{t('tasks.breakdown.count', { count: keptCount })}</span>
              </div>
              {dueAt === null ? null : (
                <div className="flex min-w-0 flex-col">
                  <span className="meta">
                    {t('tasks.breakdown.due', { when: dueLabel(dueAt, timeZone) })}
                  </span>
                </div>
              )}
            </div>

            {proposal.warnings.length > 0 ? (
              <ul className="flex flex-col gap-1">
                {proposal.warnings.map((warning) => (
                  <li key={warning} className="note note-warning wrap-anywhere">
                    {t('tasks.breakdown.warning', { detail: warning })}
                  </li>
                ))}
              </ul>
            ) : null}

            <div className="flex flex-wrap gap-2 border-t border-border pt-4">
              <button
                type="button"
                className="btn btn-primary"
                disabled={busy || keptCount === 0}
                onClick={() => void accept()}
              >
                <Icon name="check" size={20} />
                {t('tasks.breakdown.accept')}
              </button>
              <button type="button" className="btn btn-quiet" onClick={onClose}>
                {t('actions.dismiss')}
              </button>
            </div>
          </div>
        ) : null}
      </div>
    </Section>
  )
}
