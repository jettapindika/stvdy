import type { TFunction } from 'i18next'

import type { TaskRow } from './order'

/**
 * The four figures the week summary band states, and the duration formatter they
 * share with the board's columns.
 *
 * **Every figure is a count of rows the device already holds**, so each one traces
 * to a task rather than to an estimate about tasks. The band is the design's, and
 * the design's own numbers are illustrative; what is computed here is what the
 * rows say.
 *
 * **`estimateMin` is `null` when no open task carries an estimate, and that is
 * not a zero.** `estimate_min` is nullable, so "0 minutes of estimated work" would
 * be a claim the app cannot make from a set of tasks none of which has an estimate
 * — the same rule that makes `quiz_accuracy_pct` `null` rather than `0` on the
 * prep card. The band omits the metric instead, which is what the caller reads
 * `null` for.
 *
 * A count of zero *is* a real count — "0 open tasks" is a fact about a list the
 * app has — so the three counts are numbers and never absences.
 *
 * "Completed" counts rows whose status is `done`, which is a state and not a
 * timestamp: nothing in this app records *when* a task was completed, so no figure
 * here is scoped to a day or a week. That is why the band is titled "Your work"
 * and not "This week's work".
 */
export interface TaskSummary {
  /** Live rows, including subtasks: the work the student actually has. */
  total: number
  open: number
  highPriority: number
  done: number
  /** Minutes over the open tasks that carry one, or `null` when none does. */
  estimateMin: number | null
}

export function taskSummary(rows: readonly TaskRow[]): TaskSummary {
  const live = rows.filter((row) => row.deleted_at == null)
  const open = live.filter((row) => row.status !== 'done')
  const estimates = open
    .map((row) => row.estimate_min)
    .filter((minutes): minutes is number => minutes !== null)

  return {
    total: live.length,
    open: open.length,
    // `tasks.priority` is 1 high / 2 normal / 3 low (migration 0012), so "high" is
    // the number 1 and not a rank this module invents.
    highPriority: open.filter((row) => row.priority === 1).length,
    done: live.length - open.length,
    estimateMin: estimates.length === 0 ? null : estimates.reduce((sum, m) => sum + m, 0),
  }
}

/** The minutes `estimate_min` totals over one status column, or `null` when none has one. */
export function estimateTotal(rows: readonly TaskRow[]): number | null {
  const estimates = rows
    .map((row) => row.estimate_min)
    .filter((minutes): minutes is number => minutes !== null)
  return estimates.length === 0 ? null : estimates.reduce((sum, m) => sum + m, 0)
}

/**
 * A minute count as the student reads a duration.
 *
 * The unit words are the copy layer's, not this function's: Indonesian writes `5j
 * 40m` where English writes `5h 40m`, and a formatter that hardcoded the letters
 * would be a second place a language is decided.
 */
export function durationLabel(t: TFunction, minutes: number): string {
  const whole = Math.max(0, Math.trunc(minutes))
  const hours = Math.floor(whole / 60)
  const rest = whole % 60
  if (hours === 0) return t('planner.duration.m', { minutes: rest })
  if (rest === 0) return t('planner.duration.h', { hours })
  return t('planner.duration.hm', { hours, minutes: rest })
}
