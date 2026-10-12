import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { asApiError, type ApiError } from '@/api/errors'
import { deleteTask, saveTask, useTasks, type TaskInput } from '@/api/queries/tasks'
import { PageHeader, Section } from '@/components/layout/Section'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
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
  const tasks = useTasks()
  const [error, setError] = useState<ApiError | null>(null)
  const [breakdownFor, setBreakdownFor] = useState<string | null>(null)

  const rows = orderTasks(tasks.data ?? [])
  const roots = rows.filter((row) => row.parent_id === null)

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader title={t('nav.tasks')} subtitle={t('tasks.subtitle')} />

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
  onBreakdown,
  onError,
}: {
  task: TaskRow
  all: TaskRow[]
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
          {task.due_at !== null ? <span className="meta">{task.due_at}</span> : null}
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
