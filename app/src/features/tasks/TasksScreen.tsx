import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import { deleteTask, saveTask, useTasks, type TaskInput } from '@/api/queries/tasks'
import { useWorkspaces } from '@/api/queries/workspaces'
import { Section } from '@/components/layout/Section'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { Icon } from '@/components/ui/Icon'
import { useFormFactor } from '@/lib/useFormFactor'
import { BreakdownPanel } from './BreakdownPanel'
import { orderTasks, subtasksOf, type TaskRow } from './order'
import { DueLine, PriorityChip, StatusChip, SubtaskLines } from './parts'
import { durationLabel, taskSummary, type TaskSummary } from './summary'
import { TaskBoard } from './TaskBoard'

/**
 * Tasks, as a **panel of the planner**.
 *
 * It is the `Tasks` half of the design's `Calendar | Tasks` pair, and it renders
 * the panel only: the page heading, the view switcher and the footer belong to
 * `PlannerScreen`, which is what keeps one screen from having two `<h1>`s. The
 * file and the export kept their names because the tests, the accessibility walk
 * and the phase's file list all address it by this one; the role is what changed.
 *
 * **The layout is the URL's, not this component's state.** `?view=tasks&layout=…`
 * is read by the host and passed down, so the board is a place a student can link
 * to and come back to. List is the default, because the list is the view that can
 * show every column a row has.
 *
 * **Six columns are a desk layout, and the phone gets stacked cards.** A
 * six-column table cannot survive 390px at 200% text — the calendar's
 * `TimeGrid` measurement is the same finding, and it is the reason that grid is a
 * table of *rows* rather than of columns. So `useFormFactor` chooses between the
 * table and the card list, which is how the shell itself decides between the
 * sidebar and the tab bar. One of the two renders, rather than both with one
 * hidden by CSS: two DOMs would double every text pair the accessibility gate
 * counts and would let a test assert on the one the student cannot see.
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
 * the children are queued here in the same action. The control is here and *not* on
 * the board, so a mis-tap in the denser view cannot take out a parent and its
 * children.
 *
 * **A done task is still listed here, unlike on Today.** The Today view answers
 * "what must I do", and this panel answers "what do I have" — a completed task
 * that vanished from the view that can un-complete it would be a row the student
 * could not reach.
 *
 * **The summary band is `Your work`, not `This week's work`.** The design's title
 * is the latter and its figures are week-scoped, and one of the four cannot be:
 * nothing in this schema records *when* a task was completed, so "completed this
 * week" is not a number this app can compute. Every figure here is a count of the
 * rows the device already holds — open, estimated, high priority, done — and the
 * band says so rather than borrowing a window it cannot honour.
 */
export function TasksScreen({
  now,
  timeZone,
  layout,
}: {
  /** The instant the planner was opened, fixed by the host. */
  now: Date
  timeZone: string
  /** `list` or `board`, from `?layout=`. */
  layout: 'list' | 'board'
}) {
  const { t } = useTranslation()
  const isPhone = useFormFactor() === 'phone'
  const tasks = useTasks()
  const workspaces = useWorkspaces()
  const [error, setError] = useState<ApiError | null>(null)
  const [breakdownFor, setBreakdownFor] = useState<string | null>(null)
  const [query, setQuery] = useState('')

  const rows = orderTasks(tasks.data ?? [])
  const roots = rows.filter((row) => row.parent_id === null)
  const summary = taskSummary(rows)

  // The course label the design puts above every task title: `BIO 201 · Cell
  // biology`. The code is optional on the workspace (`workspaces.course_code` is
  // nullable), so a workspace without one contributes its name alone — inventing a
  // code is the one thing this app may not do with data.
  const courses: Record<string, string> = {}
  for (const workspace of workspaces.data ?? []) {
    courses[workspace.id] =
      workspace.course_code === null
        ? workspace.name
        : `${workspace.course_code} · ${workspace.name}`
  }

  // The server's course index for each workspace, so a row on the board wears the
  // same hue here as on its card and its calendar entry. The map is built from the
  // list this screen already fetches; `courseHue` keeps its id hash as the fallback
  // for a row whose workspace has not loaded.
  const workspaceColors = new Map<string, number>()
  for (const workspace of workspaces.data ?? []) {
    workspaceColors.set(workspace.id, workspace.color)
  }
  const courseCount = new Set(
    rows
      .map((row) => row.workspace_id)
      .filter((id): id is string => id !== null && id in courses),
  ).size

  // A filter over rows the device already holds, so it is instant and it works
  // offline. It searches the titles and the notes, which is what the helper line
  // under the field says, and it is case-insensitive because nobody types a title
  // the way they wrote it.
  const needle = query.trim().toLowerCase()
  const visible =
    needle === ''
      ? roots
      : roots.filter(
          (row) =>
            row.title.toLowerCase().includes(needle) ||
            (row.notes ?? '').toLowerCase().includes(needle),
        )

  const toggle = async (task: TaskRow) => {
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
      setError(asApiError(failure))
    }
  }

  const remove = async (task: TaskRow) => {
    try {
      await deleteTask(
        task.id,
        subtasksOf(rows, task.id).map((child) => child.id),
      )
    } catch (failure) {
      setError(asApiError(failure))
    }
  }

  return (
    <div className="flex w-full min-w-0 flex-col gap-6">
      {error !== null ? (
        <ApiErrorNotice error={error} noun={t('tasks.noun')} action={t('actions.retry')} />
      ) : null}

      <TaskSummaryBand summary={summary} courseCount={courseCount} />

      <TaskFilters query={query} onQuery={setQuery} />

      <Section
        title={t('tasks.list.all')}
        action={
          <p className="meta min-w-0 break-words">
            {t('tasks.list.meta', {
              count: rows.length,
              total: rows.length,
              open: summary.open,
            })}
          </p>
        }
      >
        {tasks.isPending ? (
          <LoadingState label={t('tasks.loading')} />
        ) : visible.length === 0 ? (
          needle !== '' ? (
            <EmptyState
              title={t('tasks.find.none', { query })}
              reason={t('tasks.find.helper')}
              action={
                <button type="button" className="btn btn-secondary" onClick={() => setQuery('')}>
                  {t('tasks.find.clear')}
                </button>
              }
            />
          ) : (
            <EmptyState title={t('tasks.empty.title')} reason={t('tasks.empty.reason')} />
          )
        ) : layout === 'board' && !isPhone ? (
          <TaskBoard
            rows={rows}
            timeZone={timeZone}
            now={now}
            courses={courses}
            courseColors={workspaceColors}
            onToggle={(task) => void toggle(task)}
            onBreakdown={setBreakdownFor}
            onNewTask={() => {
              document.getElementById('new-task')?.focus()
            }}
          />
        ) : (
          // The phone renders the card list for **both** layouts: a three-column
          // board is three columns of cards, and one column of cards is the same
          // information in the order a thumb reads it. Switching would be a
          // re-layout with nothing behind it.
          <TaskList
            tasks={visible}
            all={rows}
            timeZone={timeZone}
            now={now}
            courses={courses}
            table={!isPhone}
            onToggle={(task) => void toggle(task)}
            onRemove={(task) => void remove(task)}
            onBreakdown={setBreakdownFor}
          />
        )}
      </Section>

      {/* The create form is the panel's last block, so the `+ New task` control in
          the page heading has somewhere to land: the anchor is the section's own
          id and `tabIndex={-1}` is what makes the browser move focus, not only
          scroll. A heading control that scrolled nowhere would be decoration. */}
      <section id="new-task" tabIndex={-1} className="scroll-mt-4 focus:outline-none">
        <Section title={t('tasks.new.title')}>
          <CreateForm onError={setError} />
        </Section>
      </section>

      {breakdownFor !== null ? (
        // The title and the due instant come from the row the panel was opened
        // from, so the panel does not fetch the task list a second time to render
        // two lines it was already handed.
        <BreakdownPanel
          taskId={breakdownFor}
          taskTitle={rows.find((row) => row.id === breakdownFor)?.title ?? ''}
          dueAt={rows.find((row) => row.id === breakdownFor)?.due_at ?? null}
          timeZone={timeZone}
          onClose={() => setBreakdownFor(null)}
        />
      ) : null}
    </div>
  )
}

/**
 * The four figures, and the layout switch beside them.
 *
 * **A figure that cannot be computed is omitted, never zero-filled.** The
 * estimated-work figure is absent when no open task carries an estimate, because
 * `estimate_min` is nullable and "0 minutes of estimated work" would be a claim
 * about a set of tasks the app has not been told the effort of. The other three are
 * counts of rows, and a count of zero is a real count.
 */
function TaskSummaryBand({
  summary,
  courseCount,
}: {
  summary: TaskSummary
  courseCount: number
}) {
  const { t } = useTranslation()

  return (
    <div className="raised flex flex-col gap-4 border border-border p-5 lg:flex-row lg:items-center lg:justify-between">
      <div className="flex min-w-0 flex-col gap-1">
        <p className="text-label text-text">{t('planner.work.title')}</p>
        {courseCount > 0 ? (
          <p className="meta">{t('planner.work.courses', { count: courseCount })}</p>
        ) : null}
      </div>

      <div className="flex flex-wrap items-end gap-x-8 gap-y-4">
        <Metric value={String(summary.open)} label={t('planner.work.open')} />
        {summary.estimateMin === null ? null : (
          <Metric value={durationLabel(t, summary.estimateMin)} label={t('planner.work.estimated')} />
        )}
        <Metric value={String(summary.highPriority)} label={t('planner.work.high')} />
        <Metric value={String(summary.done)} label={t('planner.work.completed')} />
      </div>

    </div>
  )
}

/** One figure: a 24/32 number over its 14/20 label. */
function Metric({ value, label }: { value: string; label: string }) {
  return (
    <div className="flex min-w-0 flex-col">
      <span className="text-h2 min-w-0 break-words">{value}</span>
      <span className="text-label min-w-0 break-words text-muted">{label}</span>
    </div>
  )
}

/**
 * List or board.
 *
 * Links rather than buttons, so each layout is an address the student can come
 * back to and the accessibility walk can reach. `aria-current` marks the active
 * one and the label weight goes 500 -> 600 with it, so the state is not carried by
 * colour alone.
 *
 * **Exported, because the row it belongs to is the host's.** The frame draws this
 * switch on the right of the same row as `Calendar | Tasks`, and that row is
 * `PlannerScreen`'s — so the control is defined here, where the URL shape it writes
 * lives, and rendered there.
 */
export function LayoutSwitch({ layout }: { layout: 'list' | 'board' }) {
  const { t } = useTranslation()
  const options: { id: 'list' | 'board'; label: string; icon: 'list-checks' | 'layers' }[] = [
    { id: 'list', label: t('planner.layout.list'), icon: 'list-checks' },
    { id: 'board', label: t('planner.layout.board'), icon: 'layers' },
  ]

  return (
    <nav
      aria-label={t('planner.layout.label')}
      className="flex shrink-0 flex-wrap gap-1 rounded-field bg-surface-raised p-1"
    >
      {options.map((option) => {
        const active = option.id === layout
        return (
          <Link
            key={option.id}
            to={`/planner?view=tasks&layout=${option.id}`}
            aria-current={active ? 'page' : undefined}
            className={`flex min-h-8 min-w-0 items-center gap-2 rounded-field px-4 text-label ${
              active
                ? 'border border-border bg-surface font-semibold text-primary-text'
                : 'border border-transparent text-muted hover:text-text'
            }`}
          >
            <Icon name={option.icon} size={16} />
            {option.label}
          </Link>
        )
      })}
    </nav>
  )
}

/**
 * The filters row: a labelled search field, its helper line, and the order.
 *
 * **The order is stated, not offered.** `order.ts` mirrors the server's own
 * `order by due_at nulls last, priority, position, created_at, id`, and two orders
 * would put the same rows in two sequences on two screens. The design draws a
 * chevron beside `Sort: Due date`; there is no second order to choose, and a
 * chevron that opens nothing is a control whose only outcome is nothing.
 *
 * The three filter dropdowns the frame draws (`All courses`, `All priorities`,
 * `This week`) are likewise absent: each would be a chevron with no menu behind it.
 * The search field is the filter that does something today.
 */
function TaskFilters({ query, onQuery }: { query: string; onQuery: (value: string) => void }) {
  const { t } = useTranslation()

  return (
    <div className="flex flex-wrap items-start gap-4">
      <div className="flex min-w-0 flex-1 flex-col gap-1">
        <label className="text-label text-text" htmlFor="task-search">
          {t('tasks.find.label')}
        </label>
        <input
          id="task-search"
          className="field"
          type="search"
          value={query}
          placeholder={t('tasks.find.placeholder')}
          onChange={(event) => onQuery(event.target.value)}
        />
        {/* `break-words`: the helper is a sentence in a column narrower than its
            longest word at 200% text, and without this it overflowed by 13px. */}
        <p className="text-caption text-muted break-words">{t('tasks.find.helper')}</p>
      </div>
      <p className="meta flex min-h-11 items-center self-end">{t('tasks.sort.due')}</p>
    </div>
  )
}

/**
 * The task list.
 *
 * `table` is the form factor's answer, not a style: at a desk the six columns are
 * a comparison, and on a phone they are a stack. Both render the same facts from
 * the same pieces, so the two cannot drift into saying different things about one
 * row.
 */
function TaskList({
  tasks,
  all,
  timeZone,
  now,
  courses,
  table,
  onToggle,
  onRemove,
  onBreakdown,
}: {
  tasks: TaskRow[]
  all: TaskRow[]
  timeZone: string
  now: Date
  courses: Record<string, string>
  table: boolean
  onToggle: (task: TaskRow) => void
  onRemove: (task: TaskRow) => void
  onBreakdown: (taskId: string) => void
}) {
  const { t } = useTranslation()
  const columns = [
    t('tasks.column.task'),
    t('tasks.column.status'),
    t('tasks.column.priority'),
    t('tasks.column.estimate'),
    t('tasks.column.due'),
    t('tasks.column.subtasks'),
  ]

  if (!table) {
    return (
      <div className="card flex flex-col divide-y divide-border">
        {tasks.map((task) => (
          <TaskCard
            key={task.id}
            task={task}
            all={all}
            timeZone={timeZone}
            now={now}
            courses={courses}
            onToggle={onToggle}
            onRemove={onRemove}
            onBreakdown={onBreakdown}
          />
        ))}
      </div>
    )
  }

  return (
    <div className="card flex flex-col">
      {/* The header row is a grid of the same six fractions the rows use, so a
          column heading cannot drift away from the column under it. It is
          `aria-hidden`: every cell below carries its own content, and a screen
          reader hears the row rather than a heading it cannot associate with a
          cell (this is not a `<table>`, deliberately — the rows hold controls and
          nested lists, which is exactly the content a table cell cannot carry). */}
      <div
        aria-hidden="true"
        className="raised grid grid-cols-[minmax(0,3fr)_repeat(5,minmax(0,1fr))] gap-4 rounded-b-none border-b border-border px-5 py-2"
      >
        {columns.map((column) => (
          <span key={column} className="eyebrow min-w-0 break-words">
            {column}
          </span>
        ))}
      </div>
      <ul className="flex flex-col divide-y divide-border">
        {tasks.map((task) => (
          <li key={task.id} className="min-w-0">
            <TaskTableRow
              task={task}
              all={all}
              timeZone={timeZone}
              now={now}
              courses={courses}
              onToggle={onToggle}
              onRemove={onRemove}
              onBreakdown={onBreakdown}
            />
          </li>
        ))}
      </ul>
      <p className="meta border-t border-border px-5 py-3">
        {t('tasks.list.showing', { count: tasks.length })} · {t('tasks.list.footnote')}
      </p>
    </div>
  )
}

/** The six cells of one row, and the actions that belong to it. */
function TaskTableRow({
  task,
  all,
  timeZone,
  now,
  courses,
  onToggle,
  onRemove,
  onBreakdown,
}: {
  task: TaskRow
  all: TaskRow[]
  timeZone: string
  now: Date
  courses: Record<string, string>
  onToggle: (task: TaskRow) => void
  onRemove: (task: TaskRow) => void
  onBreakdown: (taskId: string) => void
}) {
  const { t } = useTranslation()
  const children = subtasksOf(all, task.id)
  const course = task.workspace_id === null ? undefined : courses[task.workspace_id]

  return (
    <div className="flex min-w-0 flex-col gap-3 px-5 py-4">
      <div className="grid grid-cols-[minmax(0,3fr)_repeat(5,minmax(0,1fr))] items-start gap-4">
        <div className="flex min-w-0 flex-col gap-1">
          <span className="min-w-0 break-words text-label text-text">
            {task.status === 'done' ? t('tasks.done', { title: task.title }) : task.title}
          </span>
          {course === undefined ? null : (
            <span className="meta min-w-0 break-words text-primary-text">{course}</span>
          )}
        </div>
        <StatusChip status={task.status} />
        {/* `flex`: an `inline-flex` chip inside a plain block is an atomic inline box
            that cannot be constrained, so it overflowed the column instead of wrapping
            its label. As a flex item it can shrink (`min-width: 0` on `.chip`). */}
        <span className="flex min-w-0">
          <PriorityChip priority={task.priority} />
        </span>
        <span className="meta flex min-w-0 items-center break-words">
          {task.estimate_min === null ? null : durationLabel(t, task.estimate_min)}
        </span>
        <DueLine dueAt={task.due_at} timeZone={timeZone} now={now} />
        <SubtaskLines subtasks={children} />
      </div>

      <TaskActions
        task={task}
        onToggle={onToggle}
        onRemove={onRemove}
        onBreakdown={onBreakdown}
      />
    </div>
  )
}

/** One task as a card, for a phone. The same six facts, stacked. */
function TaskCard({
  task,
  all,
  timeZone,
  now,
  courses,
  onToggle,
  onRemove,
  onBreakdown,
}: {
  task: TaskRow
  all: TaskRow[]
  timeZone: string
  now: Date
  courses: Record<string, string>
  onToggle: (task: TaskRow) => void
  onRemove: (task: TaskRow) => void
  onBreakdown: (taskId: string) => void
}) {
  const { t } = useTranslation()
  const children = subtasksOf(all, task.id)
  const course = task.workspace_id === null ? undefined : courses[task.workspace_id]

  return (
    <div className="flex min-w-0 flex-col gap-3 p-5">
      {course === undefined ? null : (
        <p className="meta min-w-0 break-words text-primary-text">{course}</p>
      )}
      <p className="min-w-0 break-words text-label text-text">
        {task.status === 'done' ? t('tasks.done', { title: task.title }) : task.title}
      </p>
      <div className="flex flex-wrap items-center gap-3">
        <StatusChip status={task.status} />
        <PriorityChip priority={task.priority} />
        {task.estimate_min === null ? null : (
          <span className="meta rounded-field bg-surface-raised px-2 py-1">
            {durationLabel(t, task.estimate_min)}
          </span>
        )}
      </div>
      <DueLine dueAt={task.due_at} timeZone={timeZone} now={now} />
      <SubtaskLines subtasks={children} />
      <TaskActions
        task={task}
        onToggle={onToggle}
        onRemove={onRemove}
        onBreakdown={onBreakdown}
      />
    </div>
  )
}

/** Mark done / reopen, break down, delete. The row's own controls, in one place. */
function TaskActions({
  task,
  onToggle,
  onRemove,
  onBreakdown,
}: {
  task: TaskRow
  onToggle: (task: TaskRow) => void
  onRemove: (task: TaskRow) => void
  onBreakdown: (taskId: string) => void
}) {
  const { t } = useTranslation()
  return (
    <div className="flex flex-wrap gap-2">
      <button type="button" className="btn btn-secondary" onClick={() => onToggle(task)}>
        {task.status === 'done' ? t('tasks.reopen') : t('tasks.markDone')}
      </button>
      <button type="button" className="btn btn-quiet" onClick={() => onBreakdown(task.id)}>
        {t('tasks.breakdown.action')}
      </button>
      <button type="button" className="btn btn-quiet" onClick={() => onRemove(task)}>
        {t('actions.delete')}
      </button>
    </div>
  )
}

/** The create form: a title, and the date worth asking for up front. */
function CreateForm({ onError }: { onError: (error: ApiError | null) => void }) {
  const { t } = useTranslation()
  const [title, setTitle] = useState('')
  const [dueAt, setDueAt] = useState('')

  const submit = async () => {
    try {
      const input: TaskInput = {
        id: crypto.randomUUID(),
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
      onError(null)
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
          control is for, and the phone's sideways scroll is prevented at the
          screen root rather than by squeezing this box. Do not "fix" it here with
          `max-width: 0` — measured, that leaves a 59.6px empty control with no
          date and no picker. The `max-w-48` is what keeps it from taking the whole
          row at a desk; on a phone the field's own `min-width: 0` in styles.css is
          what lets it shrink. */}
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
