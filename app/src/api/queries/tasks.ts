import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'
import { platform } from '@/platform'
import type { TaskRow } from '@/features/tasks/order'
import { hasLocalCache } from './synced'

/**
 * Tasks: the list, the writes, and the two generative actions.
 *
 * Three things this module is the single home of.
 *
 * **Which backend.** A build with a cache reads and writes through
 * `platform.sync` (the cache and the outbox); the web build uses the REST routes.
 * Both end at the same table triggers and the same `change_log` entries, so this
 * is a transport choice and not a second write path.
 *
 * **The id.** A create mints its own id (A8). That is what makes an offline
 * create real before the network exists: the row has an identity on the device,
 * the outbox carries it, and a retry cannot create a second task.
 *
 * **What a write carries.** An update sends the whole row rather than the changed
 * fields. `PATCH /tasks/:id` is partial, but the outbox stores the payload it was
 * given and the next device to pull gets a record-level LWW merge — so a payload
 * naming one field would replace the row's other columns locally with nothing
 * until the server's answer came back.
 */

export interface QuickCaptureDraft {
  title: string
  notes: string | null
  due_at: string | null
  priority: number
  estimate_min: number | null
  /** One line per field the reply could not supply as asked (A194). */
  warnings: string[]
}

export interface ProposedSubtask {
  title: string
  estimate_min: number
}

export interface BreakdownProposal {
  subtasks: ProposedSubtask[]
  total_estimate_min: number
  warnings: string[]
}

export interface Credits {
  credits_spent: number
  credits_remaining: number
  prompt_version: string
}

/** A task write, in the shape both paths accept. */
export interface TaskInput {
  id: string
  workspace_id: string | null
  parent_id: string | null
  title: string
  notes: string | null
  due_at: string | null
  estimate_min: number | null
  priority: number
  status: 'todo' | 'doing' | 'done'
  position: number
}

export const taskKeys = {
  all: ['tasks'] as const,
}

/** Every task this device can see, tombstone-free, in no particular order. */
async function fetchTasks(): Promise<TaskRow[]> {
  if (hasLocalCache()) {
    const rows = await platform.sync.read<TaskRow>('task')
    return rows.filter((row) => row.deleted_at == null)
  }
  const body = await api.get<{ tasks: TaskRow[] }>('/tasks')
  return body.tasks
}

export function useTasks() {
  return useQuery({ queryKey: taskKeys.all, queryFn: fetchTasks })
}

/**
 * Creates or updates a task.
 *
 * `isNew` is the caller's statement about intent, and it matters on the REST path
 * only: `POST /tasks` refuses an id that is taken (409) and `PATCH` refuses one
 * that is not there (404), so guessing wrong is a failure rather than a silent
 * upsert. On the cache path both are the same write.
 */
export async function saveTask(input: TaskInput, isNew: boolean): Promise<void> {
  const payload = { ...input }
  if (!hasLocalCache()) {
    if (isNew) await api.post<{ task: unknown }>('/tasks', payload)
    else await api.patch<{ task: unknown }>(`/tasks/${input.id}`, payload)
    return
  }
  await platform.sync.write('task', input.id, payload)
}

/**
 * Tombstones a task and its live descendants.
 *
 * The children are queued here rather than left to the server, because the
 * cascade only fires when the server applies the parent's delete: a device that
 * deleted a parent while offline would show orphaned subtasks until the next pull
 * (A197).
 */
export async function deleteTask(id: string, childIds: string[]): Promise<void> {
  if (!hasLocalCache()) {
    await api.delete(`/tasks/${id}`)
    return
  }
  for (const childId of childIds) await platform.sync.tombstone('task', childId)
  await platform.sync.tombstone('task', id)
}

/**
 * `POST /tasks/quick-capture`: a draft, and **no task** (A193).
 *
 * Never through the outbox: the model call is the server's, so this is the one
 * action here that cannot work offline, and its refusal is the plan's
 * `plan_required` rather than a queued mutation.
 */
export async function quickCapture(text: string): Promise<QuickCaptureDraft & Credits> {
  const body = await api.post<{ draft: QuickCaptureDraft } & Credits>('/tasks/quick-capture', {
    text,
  })
  return { ...body.draft, ...body }
}

/**
 * `POST /tasks/:id/breakdown`: a proposal, and **no subtasks** (A195).
 *
 * Accepting the proposal is N ordinary creates with `parent_id` and `position` —
 * this route never writes one, which is what makes Dismiss free and keeps a
 * proposal from being a plan the student did not agree to.
 */
export async function breakdownTask(id: string): Promise<BreakdownProposal & Credits> {
  const body = await api.post<{ proposal: BreakdownProposal } & Credits>(`/tasks/${id}/breakdown`)
  return { ...body.proposal, ...body }
}

/** Invalidates the task list after any write. */
export function useTaskWrite() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: ({ input, isNew }: { input: TaskInput; isNew: boolean }) => saveTask(input, isNew),
    onSuccess: () => client.invalidateQueries({ queryKey: taskKeys.all }),
  })
}
