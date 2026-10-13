import { useTranslation } from 'react-i18next'

import { Icon } from '@/components/ui/Icon'
import { formatInZone } from '@/lib/time'
import type { TaskRow, TaskStatus } from './order'

/**
 * The pieces a task row is made of, shared by the list and the board.
 *
 * Both views render the same facts about the same rows, and only their *layout*
 * differs: the list puts them in six columns at a desk and in a stack on a phone,
 * and the board puts them in a card. Two copies of the priority chip would be two
 * places to get the 1/2/3 scale wrong, and the scale is a real one rather than a
 * presentation choice — `tasks.priority` is `1 high, 2 normal, 3 low` (migration
 * 0012), so the mapping from number to word lives here once.
 *
 * **Every chip carries its word.** The tone repeats the state and never replaces
 * it (gate 5d), which is also why the fallback for an out-of-range priority is the
 * normal word rather than a blank chip: a row whose number the app does not
 * recognise still has to say something true.
 */

/** `Fri 16 Oct · 23:59`, in the student's own zone. */
export function dueLabel(dueAt: string, timeZone: string): string {
  const instant = new Date(dueAt)
  const date = formatInZone(instant, timeZone, { weekday: 'short', day: 'numeric', month: 'short' })
  const time = formatInZone(instant, timeZone, { hour: '2-digit', minute: '2-digit', hour12: false })
  return `${date} · ${time}`
}

/** The status word. To do / In progress / Done, with a marker whose tone repeats it. */
export function StatusChip({ status }: { status: TaskStatus }) {
  const { t } = useTranslation()
  const tone =
    status === 'done' ? 'bg-success' : status === 'doing' ? 'bg-primary' : 'bg-muted'
  return (
    <span className="flex min-w-0 items-center gap-2">
      <span aria-hidden="true" className={`h-1.5 w-1.5 shrink-0 rounded ${tone}`} />
      <span className="min-w-0 break-words text-label">{t(`tasks.status.${status}`)}</span>
    </span>
  )
}

/** The priority as a chip. The word is always present; the tone only repeats it. */
export function PriorityChip({ priority }: { priority: number }) {
  const { t } = useTranslation()
  // **The label goes in its own span, and that is not decoration.** `.chip` is an
  // `inline-flex` container, so bare text becomes an *anonymous* flex item whose
  // automatic minimum size is its longest word — at 200% text "Medium" measures 131px
  // and refuses to shrink into the task row's 76px column, so it was clipped. A real
  // child element can be given `min-w-0` and `break-words`, which is the pattern
  // `StatusChip` above and `DueLine` below already use for the same reason.
  const label =
    priority === 1
      ? t('tasks.priority.high')
      : priority === 3
        ? t('tasks.priority.low')
        : t('tasks.priority.medium')
  const tone = priority === 1 ? ' chip-due' : priority === 3 ? ' chip-ready' : ''
  return (
    <span className={`chip${tone}`}>
      <span className="min-w-0 break-words">{label}</span>
    </span>
  )
}

/** The due line: a calendar glyph and the instant, in the student's zone. */
export function DueLine({ dueAt, timeZone, now }: { dueAt: string | null; timeZone: string; now: Date }) {
  if (dueAt === null) return null
  // Measured against the instant the view was opened, so the answer cannot change
  // while the student is looking at the row.
  const overdue = Date.parse(dueAt) < now.getTime()
  return (
    <span
      className={`flex min-w-0 items-center gap-2 ${overdue ? 'text-warning' : 'text-muted'}`}
    >
      <Icon name="calendar" size={16} />
      <span className="meta min-w-0 break-words">{dueLabel(dueAt, timeZone)}</span>
    </span>
  )
}

/**
 * A parent's subtasks, one line each, with the done ones carrying the word.
 *
 * A count rather than a bar: the number of done children is the fact, and a
 * percentage would be a second number this screen computed. The done state is a
 * word (`tasks.done`) because a checkbox that does not toggle is a control whose
 * only outcome is nothing — the toggle lives on the parent's own action row.
 */
export function SubtaskLines({ subtasks }: { subtasks: TaskRow[] }) {
  const { t } = useTranslation()
  const done = subtasks.filter((child) => child.status === 'done').length

  if (subtasks.length === 0) {
    return <span className="meta break-words">{t('tasks.subtasks.none')}</span>
  }

  return (
    <span className="flex min-w-0 flex-col gap-1">
      <span className="meta break-words">
        {t('tasks.subtasks.count', { done, total: subtasks.length })}
      </span>
      <span className="flex min-w-0 flex-col gap-1">
        {subtasks.map((child) => (
          <span key={child.id} className="min-w-0 break-words text-label text-muted">
            {child.status === 'done' ? t('tasks.done', { title: child.title }) : child.title}
          </span>
        ))}
      </span>
    </span>
  )
}
