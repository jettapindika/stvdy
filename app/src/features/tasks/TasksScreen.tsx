import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import { deleteTask, saveTask, useTasks, type TaskInput } from '@/api/queries/tasks'
import { PageHeader, Section } from '@/components/layout/Section'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { BreakdownPanel } from './BreakdownPanel'
import { orderTasks, subtasksOf, type TaskRow } from './order'

/**
 * Tasks: the list, the create form, and the break-down panel.
 *
 * **Subtasks are one level, and the parent shows its progress as a count.** The
 * tree is walked with `order.ts`'s rules — the same order the server sorts by, so
 * a task the student just created does not jump to a different place on the next
 * pull. A subtask is not rendered under a subtask: the depth the spec asks for is
 * one, and a grandchild is its own parent's child.
 *
 * **Deleting a parent tombstones its children on this device too.** The server's
 * cascade fires when *it* applies the parent's delete, and a device that deleted a
 * parent while offline would show orphaned subtasks until the next pull (A197), so
 * the children are queued here in the same action.
 *
 * **A done task is still listed here, unlike on Today.** The Today view answers
 * "what must I do", and the Tasks screen answers "what do I have" — a completed
 * task that vanished from the screen that can un-complete it would be a row the
 * student could not reach.
 */
export function TasksScreen() {
  const { t } = useTranslation()
  const session = useSession()
  const tasks = useTasks()
  // The student's own zone, which is where a due date has to be rendered: `due_at`
  // is a `timestamptz` and the row showed its raw UTC instant, so a task due 23:59
  // WIB read `2026-10-16T16:59:00Z` — the same defect class as `QuickCapture`'s Due
  // line and the study screen's header link, found by the gate's `tasks` surface
  // capturing the literal string as a text pair.
  const timeZone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE
  const [error, setError] = useState<ApiError | null>(null)
  const [breakdownFor, setBreakdownFor] = useState<string | null>(null)

  const rows = orderTasks(tasks.data ?? [])
  const roots = rows.filter((row) => row.parent_id === null)

  // `w-full` is load-bearing, and it is what Phase 14's fix-round-1 measured at
  // 200% text on a 390px phone: without it `document.scrollingElement.scrollWidth`
  // is **493** against a 390 viewport.
  //
  // The width is not the due-date control's fault, which is why the control is
  // untouched. `#task-due` is a `<input type="date">` whose *min-content* is
  // **346.6px** at 200% text — Chromium's UA stylesheet gives the date control's
  // shadow DOM an intrinsic width no author CSS can reach (in-browser probes:
  // `max-width: none`, `width: auto`, `min-width: 0`, `max-width: 100%`,
  // `flex: 0 1 auto` and `appearance: none` all leave 493; only `max-width: 0`
  // works, and it squeezes the control to a 59.6px empty box, which is a worse
  // defect than the scroll). That minimum propagates up: the card's min-content is
  // 412.6px and this root's is 492.6px.
  //
  // This div is `width: auto` inside a column flex container, so its used width is
  // its *fit-content* — and fit-content can never go below min-content, so the root
  // stays 492.6px wide inside a 390px `main` and the page scrolls. `w-full` makes
  // the width definite (the container's content box, 390px) instead of content-based,
  // which is the property that stops any wide intrinsic child from resizing the
  // screen. Measured after, both themes: **390/390**, and the control intact and
  // usable at **192x44** (100% text) and **244x88** (200%).
  //
  // `min-w-0` alone does not do it — `min-width: auto` is already 0 in the cross
  // axis; the width is what has to stop being automatic. `align-self: stretch` alone
  // does not either, measured.
  //
  // **What this does not fix, measured rather than left implicit:** at 200% text the
  // control's content box is 184.4px and the value is 168.2px in the control's own
  // font, so the picker indicator no longer has room beside it and Chromium paints
  // the indicator over the year's last digit. At 100% text there is room (84.1px of
  // value in 161.2px). The squeeze is the screen's padding at that scale — `px-5`
  // plus the card's `p-4` is 144px of a 390px viewport — which is house-wide and not
  // this screen's to change; a Tasks-local padding would be a second convention for
  // the same thing.
  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader title={t('nav.tasks')} subtitle={t('tasks.subtitle')} />

      {/* The reciprocal half of the spec's `Plan | Tasks` control.
       *
       * **Without this, `/planner` and `/syllabus` were reachable only by typing a
       * URL.** The two inbound links formed a closed cycle — `PlannerScreen` links to
       * `/syllabus` from its empty-goals state, and `SyllabusReviewScreen` links back
       * to `/planner` after a confirm — and neither nav has a planner item (the tab
       * bar's five destinations are fixed, and A189 records why a sixth would take
       * width from all of them). So a student could not open the planner, import a
       * syllabus or reach the review screen at all, and `nav.planner` existed in both
       * locales as `PlannerScreen`'s own page title only.
       *
       * `Plan` here rather than a nav item, because that is the pair the spec asks for
       * and because the planner already renders its own half: one control, two
       * screens, each linking to the other. It is also what makes Phase 11's gate-5
       * leg measurable — the walk navigates by tapping, so a screen nothing links to
       * cannot be walked at all. */}
      <nav aria-label={t('planner.views.label')} className="flex flex-wrap gap-2">
        <span className="btn btn-secondary" aria-current="page">
          {t('planner.views.tasks')}
        </span>
        <Link to="/planner" className="btn btn-quiet">
          {t('planner.views.plan')}
        </Link>
      </nav>

      {error !== null ? (
        <ApiErrorNotice error={error} noun={t('tasks.noun')} action={t('actions.retry')} />
      ) : null}

      <Section title={t('tasks.new.title')}>
        <CreateForm
          onError={setError}
          onCreated={(id) => {
            setError(null)
            void id
          }}
        />
      </Section>

      <Section title={t('tasks.list.title')} count={roots.length > 0 ? roots.length : undefined}>
        <div className="card p-4">
          {tasks.isPending ? (
            <LoadingState label={t('tasks.loading')} />
          ) : roots.length === 0 ? (
            <EmptyState
              title={t('tasks.empty.title')}
              reason={t('tasks.empty.reason')}
              padded={false}
            />
          ) : (
            <ul className="flex flex-col gap-4">
              {roots.map((task) => (
                <TaskItem
                  key={task.id}
                  task={task}
                  all={rows}
                  timeZone={timeZone}
                  onBreakdown={() => setBreakdownFor(task.id)}
                  onError={setError}
                />
              ))}
            </ul>
          )}
        </div>
      </Section>

      {breakdownFor !== null ? (
        <BreakdownPanel taskId={breakdownFor} onClose={() => setBreakdownFor(null)} />
      ) : null}
    </div>
  )
}

/** One task, its subtasks, and the actions that belong to it. */
function TaskItem({
  task,
  all,
  timeZone,
  onBreakdown,
  onError,
}: {
  task: TaskRow
  all: TaskRow[]
  // Passed rather than read here: one `useSession()` for the list, not one per row.
  timeZone: string
  onBreakdown: () => void
  onError: (error: ApiError | null) => void
}) {
  const { t } = useTranslation()
  const children = subtasksOf(all, task.id)
  const done = children.filter((child) => child.status === 'done').length

  const toggle = async () => {
    try {
      await saveTask(
        {
          id: task.id,
          workspace_id: task.workspace_id,
          parent_id: task.parent_id,
          title: task.title,
          notes: task.notes,
          due_at: task.due_at,
          estimate_min: task.estimate_min,
          priority: task.priority,
          status: task.status === 'done' ? 'todo' : 'done',
          position: task.position,
        },
        false,
      )
    } catch (failure) {
      onError(asApiError(failure))
    }
  }

  const remove = async () => {
    try {
      await deleteTask(task.id, children.map((child) => child.id))
    } catch (failure) {
      onError(asApiError(failure))
    }
  }

  return (
    <li className="flex flex-col gap-2">
      <div className="flex flex-wrap items-baseline justify-between gap-2">
        <span className="text-body">
          {task.status === 'done' ? t('tasks.done', { title: task.title }) : task.title}
        </span>
        <span className="flex flex-wrap items-center gap-2">
          {task.due_at !== null ? (
            <span className="meta">
              {t('tasks.due', {
                when: formatInZone(new Date(task.due_at), timeZone, { dateStyle: 'medium' }),
              })}
            </span>
          ) : null}
          {children.length > 0 ? (
            // A count, not a bar: the number of done children is the fact, and a
            // percentage would be a second number this screen computed.
            <span className="chip">
              {t('tasks.subtaskProgress', { done, total: children.length })}
            </span>
          ) : null}
        </span>
      </div>

      {children.length > 0 ? (
        <ul className="flex flex-col gap-1 border-l border-border pl-3">
          {children.map((child) => (
            <li key={child.id} className="text-small text-muted">
              {child.status === 'done' ? t('tasks.done', { title: child.title }) : child.title}
            </li>
          ))}
        </ul>
      ) : null}

      <div className="flex flex-wrap gap-2">
        <button type="button" className="btn btn-secondary" onClick={() => void toggle()}>
          {task.status === 'done' ? t('tasks.reopen') : t('tasks.markDone')}
        </button>
        <button type="button" className="btn btn-quiet" onClick={onBreakdown}>
          {t('tasks.breakdown.action')}
        </button>
        <button type="button" className="btn btn-quiet" onClick={() => void remove()}>
          {t('actions.delete')}
        </button>
      </div>
    </li>
  )
}

/** The create form: a title, and the two fields worth asking for up front. */
function CreateForm({
  onError,
  onCreated,
}: {
  onError: (error: ApiError | null) => void
  onCreated: (id: string) => void
}) {
  const { t } = useTranslation()
  const [title, setTitle] = useState('')
  const [dueAt, setDueAt] = useState('')

  const submit = async () => {
    try {
      const id = crypto.randomUUID()
      const input: TaskInput = {
        id,
        workspace_id: null,
        parent_id: null,
        title: title.trim(),
        notes: null,
        // A date-only field is read as local midnight in the *browser's* zone,
        // which is the zone the student typed it in. The zone is not sent: the
        // server stores an instant, and `Date` has already resolved the offset.
        due_at: dueAt === '' ? null : new Date(dueAt).toISOString(),
        estimate_min: null,
        priority: 2,
        status: 'todo',
        position: 0,
      }
      await saveTask(input, true)
      setTitle('')
      setDueAt('')
      onCreated(id)
    } catch (failure) {
      onError(asApiError(failure))
    }
  }

  return (
    <div className="card flex flex-col gap-3 p-4">
      <label className="label" htmlFor="task-title">
        {t('tasks.new.titleLabel')}
      </label>
      <input
        id="task-title"
        className="field"
        value={title}
        onChange={(event) => setTitle(event.target.value)}
      />
      <label className="label" htmlFor="task-due">
        {t('tasks.new.dueLabel')}
      </label>
      {/* `type="date"` stays: the native picker is the platform behaviour this
          control is for, and the screen's sideways scroll is fixed at the root
          above rather than by squeezing this box. Do not "fix" it here with
          `max-width: 0` — measured, that leaves a 59.6px empty control with no
          date and no picker. */}
      <input
        id="task-due"
        className="field max-w-48"
        type="date"
        value={dueAt}
        onChange={(event) => setDueAt(event.target.value)}
      />
      <button
        type="button"
        className="btn btn-primary self-start"
        disabled={title.trim().length === 0}
        onClick={() => void submit()}
      >
        {t('tasks.new.submit')}
      </button>
    </div>
  )
}
