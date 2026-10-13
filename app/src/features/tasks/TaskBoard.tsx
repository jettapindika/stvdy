import { useTranslation } from 'react-i18next'

import { Icon } from '@/components/ui/Icon'
import { courseHue } from '@/features/calendar/calendar'
import { courseVars } from '@/tokens/tokens'
import { subtasksOf, type TaskRow, type TaskStatus } from './order'
import { DueLine, PriorityChip, SubtaskLines } from './parts'
import { durationLabel, estimateTotal } from './summary'

/**
 * The Tasks board: the same rows as the list, in three status columns.
 *
 * **This is a view, not a second place to edit.** Every control here is one the
 * list already owns — mark done / reopen, break down, and the create form's
 * trigger — and each is wired to the panel's own handler rather than writing
 * itself, so the board cannot disagree with the list about what a row is.
 *
 * **There is deliberately no delete control, and no ellipsis menu.** The list
 * owns deleting a task because deleting a parent tombstones its live children in
 * the same action (A197); a mis-tap on a dense board would take out a parent and
 * every subtask under it. The frame draws an ellipsis on each column heading and
 * each card, but there is nothing behind it, and a control whose only outcome is
 * nothing is worse than its absence.
 *
 * **Subtasks are one level.** A card shows the children of its own task and no
 * grandchildren — the depth the spec asks for — and a child is never rendered as
 * a card of its own with its own controls. A child's done state is a word
 * (`tasks.done`), the same treatment the list gives it: the frame draws a
 * checkbox, but a checkbox that does not toggle is a dead control, and the
 * toggle lives on the parent's card.
 *
 * **Every figure is one the rows carry.** A column's count is its root rows; its
 * effort line is `estimateTotal` over those rows, and it is *omitted* rather than
 * printed as `0m` when no row carries an estimate — the same absence rule the
 * prep card follows. The Done column's note carries no count: nothing in this app
 * records *when* a task was completed, so "finished today" is not computable.
 */
export function TaskBoard({
  rows,
  timeZone,
  now,
  courses,
  courseColors,
  onToggle,
  onBreakdown,
  onNewTask,
}: {
  /** Every live row, already in `orderTasks` order. */
  rows: TaskRow[]
  /** The student's IANA zone: a due instant is rendered where it falls, not in UTC. */
  timeZone: string
  /**
   * The instant the view was opened, fixed by the caller.
   *
   * Passed rather than read here, because "is this overdue" is a clock read and a
   * component that made its own would re-decide the answer on every render — a
   * card could go from overdue to not while the student is looking at it, and two
   * cards on the same screen could disagree. It is the same rule the calendar and
   * the proposal grid follow.
   */
  now: Date
  /** Workspace id to "BIO 201 · Cell biology", the code omitted when there is none. */
  courses: Record<string, string>
  /**
   * Workspace id to the server's 0-5 course index, from the list the screen holds.
   *
   * Passed so a card's hue is the course's own, not a hash of its id — the same
   * reason the calendar grid builds this map. `courseHue` keeps the hash for a row
   * whose workspace has not loaded, where some stable colour beats none.
   */
  courseColors: ReadonlyMap<string, number>
  onToggle: (task: TaskRow) => void
  onBreakdown: (taskId: string) => void
  onNewTask: () => void
}) {
  const { t } = useTranslation()

  // Roots only: a subtask belongs to its parent's card. The rows are already live
  // and already ordered, so this is a filter and not a second sort.
  const roots = rows.filter((row) => row.parent_id === null && row.deleted_at == null)

  return (
    <div className="grid grid-cols-1 gap-6 md:grid-cols-3">
      {COLUMN_ORDER.map((status) => {
        const columnRows = roots.filter((row) => row.status === status)
        const effort = estimateTotal(columnRows)

        return (
          <section
            key={status}
            aria-label={t(`tasks.status.${status}`)}
            className="raised flex min-w-0 flex-col gap-3 border border-border p-3"
          >
            <header className="flex flex-col gap-2 p-2">
              <div className="flex flex-wrap items-center gap-2">
                <span
                  aria-hidden="true"
                  className={`h-2 w-2 shrink-0 rounded ${MARKER[status]}`}
                />
                <h3 className="text-h4 min-w-0 break-words">{t(`tasks.status.${status}`)}</h3>
                {/* A count of zero is a real count, so it is always shown. */}
                <span className="meta rounded-full bg-surface px-2 py-1">{columnRows.length}</span>
              </div>
              {/* Absent, not zero: `estimateTotal` is null when no row carries an
                  estimate, and the line is omitted rather than printing `0m`. */}
              {effort === null ? null : status === 'done' ? (
                <p className="meta">
                  {t('tasks.board.effortDone', { duration: durationLabel(t, effort) })}
                </p>
              ) : (
                <p className="meta">
                  {t('tasks.board.effortOpen', { duration: durationLabel(t, effort) })}
                </p>
              )}
            </header>

            {columnRows.length === 0 ? (
              <p className="p-2 text-small text-subtle">{t('tasks.board.empty')}</p>
            ) : (
              columnRows.map((task) => (
                <TaskCard
                  key={task.id}
                  task={task}
                  rows={rows}
                  timeZone={timeZone}
                  now={now}
                  courses={courses}
                  courseColors={courseColors}
                  onToggle={onToggle}
                  onBreakdown={onBreakdown}
                />
              ))
            )}

            {/* The create form always mints a `todo` row, so only To do offers it:
                a control in another column would land the task somewhere else. */}
            {status === 'todo' ? (
              <button type="button" className="btn btn-secondary" onClick={onNewTask}>
                <Icon name="plus" size={20} />
                {t('tasks.new.submit')}
              </button>
            ) : null}

            {/* No count here: nothing records when a task was completed, so "one
                finished today" is not a figure the app can compute. */}
            {status === 'done' && columnRows.length > 0 ? (
              <div className="flex flex-col gap-1 p-2">
                <p className="text-label text-success">{t('tasks.board.doneNote')}</p>
                <p className="text-small text-muted">{t('tasks.board.doneNoteHint')}</p>
              </div>
            ) : null}
          </section>
        )
      })}
    </div>
  )
}

const COLUMN_ORDER: TaskStatus[] = ['todo', 'doing', 'done']

/** The status marker's fill. The word beside it is what carries the state. */
const MARKER: Record<TaskStatus, string> = {
  todo: 'bg-muted',
  doing: 'bg-primary',
  done: 'bg-success',
}

/** One task and its subtasks. Its controls are the panel's, not its own. */
function TaskCard({
  task,
  rows,
  timeZone,
  now,
  courses,
  courseColors,
  onToggle,
  onBreakdown,
}: {
  task: TaskRow
  rows: TaskRow[]
  timeZone: string
  now: Date
  courses: Record<string, string>
  courseColors: ReadonlyMap<string, number>
  onToggle: (task: TaskRow) => void
  onBreakdown: (taskId: string) => void
}) {
  const { t } = useTranslation()
  const children = subtasksOf(rows, task.id)
  const hue = courseHue(task.workspace_id, courseColors)
  // No invented code and no placeholder: a workspace with no name renders nothing.
  const course = task.workspace_id === null ? undefined : courses[task.workspace_id]

  return (
    <article
      className="card flex min-w-0 flex-col gap-4 p-5"
      // Sets `--course-marker` / `--course-tint` for this card only.
      style={hue === null ? undefined : courseVars(hue)}
    >
      {course === undefined ? null : (
        <p className="meta min-w-0 break-words text-[color:var(--course-marker)]">{course}</p>
      )}

      <h4 className="text-h4 min-w-0 break-words">{task.title}</h4>

      <div className="flex flex-wrap items-center gap-2">
        <PriorityChip priority={task.priority} />
        {task.estimate_min === null ? null : (
          <span className="meta rounded-field bg-surface-raised px-2 py-1">
            {durationLabel(t, task.estimate_min)}
          </span>
        )}
      </div>

      <DueLine dueAt={task.due_at} timeZone={timeZone} now={now} />

      <div aria-hidden="true" className="h-px bg-border" />

      {children.length > 0 ? (
        <div className="flex min-w-0 flex-col gap-1">
          <SubtaskLines subtasks={children} />
        </div>
      ) : (
        <button
          type="button"
          className="btn btn-secondary self-start"
          onClick={() => onBreakdown(task.id)}
        >
          <Icon name="sparkles" size={20} />
          {t('tasks.breakdown.action')}
        </button>
      )}

      <div className="flex flex-wrap gap-2">
        {/* The panel owns the write; the board only asks for it. No delete control
            here — the list owns it, so a mis-tap cannot tombstone a parent and
            its children. */}
        <button type="button" className="btn btn-quiet" onClick={() => onToggle(task)}>
          {task.status === 'done' ? t('tasks.reopen') : t('tasks.markDone')}
        </button>
      </div>
    </article>
  )
}

