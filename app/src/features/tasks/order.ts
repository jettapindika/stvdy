/**
 * The one order the Tasks screen and the Today view both use.
 *
 * This mirrors `stvdy_core::tasks::order_tasks`, and it exists for the same
 * reason that function does: the server sorts in SQL, a device that is offline
 * sorts what it already holds, and two orders would put the same rows in two
 * sequences on two screens. The server's own statement names this function's
 * counterpart in a comment, so an edit to one is visibly an edit to both.
 *
 * The order is: due first with the undated last, then priority, then position,
 * then `created_at`, then the id. The id breaks every tie — two tasks can share a
 * due date to the second and a batch insert shares `created_at` to the
 * microsecond, which is exactly how Phase 5's questions came back shuffled
 * (A196, A100). A total order is what lets two devices that hold the same rows
 * put them in the same sequence without talking to each other.
 *
 * Comparisons on the id are code-unit comparisons, not `localeCompare`: a
 * collation is locale-dependent, and an order that changed with the browser's
 * language would be a different order on the same device after a language change.
 */

export type TaskStatus = 'todo' | 'doing' | 'done'

/** A task as the wire carries it. */
export interface TaskRow {
  id: string
  workspace_id: string | null
  parent_id: string | null
  title: string
  notes: string | null
  due_at: string | null
  estimate_min: number | null
  priority: number
  status: TaskStatus
  /** The order a breakdown proposed (A196); a plain task field. */
  position: number
  created_at?: string | null
  updated_at?: string
  deleted_at?: string | null
}

/** The order key: due (undated last), priority, position, `created_at`, id. */
function orderKey(row: TaskRow): [number, number, number, number, number, string] {
  const dated = row.due_at === null ? 1 : 0
  const due = row.due_at === null ? 0 : Date.parse(row.due_at)
  const created = row.created_at == null ? 0 : Date.parse(row.created_at)
  return [dated, due, row.priority, row.position, created, row.id]
}

/** The rows in the order the Tasks screen and the Today view both show them. */
export function orderTasks(rows: TaskRow[]): TaskRow[] {
  return [...rows].sort((left, right) => {
    const a = orderKey(left)
    const b = orderKey(right)
    for (let index = 0; index < a.length; index += 1) {
      const one = a[index]
      const other = b[index]
      if (one === other) continue
      if (typeof one === 'string' || typeof other === 'string') {
        return String(one) < String(other) ? -1 : 1
      }
      return (one ?? 0) < (other ?? 0) ? -1 : 1
    }
    return 0
  })
}

/**
 * Live subtasks of `parent`, in the order above.
 *
 * One level, which is the depth the UI shows and the depth the spec asks for: a
 * grandchild belongs to its own parent and is not returned here. A tombstoned
 * child is absent, so a deleted subtask does not reappear under its parent.
 */
export function subtasksOf(rows: TaskRow[], parentId: string): TaskRow[] {
  return orderTasks(
    rows.filter((row) => row.parent_id === parentId && row.deleted_at == null),
  )
}
