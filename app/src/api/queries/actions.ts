import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'

/**
 * The proposals an agent turn left for a human to confirm.
 *
 * The list is the server's, and it is deliberately **not** read from the synced
 * `pending_actions` cache on a build that has one. The card's whole job is to
 * render the truth about a row the server owns — a proposal that expired, or that
 * the student already confirmed on their other phone, must not still look
 * confirmable here — and the local copy is only as fresh as the last pull. The
 * server's lazy expiry (A274) and the single-winner claim (A275) both live on
 * that route, so this is the one read where a cache would be a lie.
 *
 * A confirm and a reject are dedicated endpoints rather than outbox writes, and
 * that is a contract fact rather than an implementation choice:
 * `Entity::PendingAction` is excluded from `is_client_writable()`, so a client
 * cannot push a resolution at all. Offline, both buttons fail, and the copy says
 * so instead of queueing work the server would never receive.
 */

/** One proposal, as `GET /actions/pending` returns it. */
export interface ActionRow {
  id: string
  /** The tool's name, e.g. `create_task`. Read as a key, never as a sentence. */
  tool: string
  /** What the model proposed, verbatim. The card renders this and nothing derived. */
  args: Record<string, unknown>
  /** The server's own status: `pending`, `confirmed`, `rejected`, `expired`. */
  status: string
  created_at: string | null
  expires_at: string
  resolved_at: string | null
  resolved_by_device_id: string | null
  /** What the execution produced, once it ran. Null while it has not. */
  result: Record<string, unknown> | null
  /** Why a confirmed proposal did not finish, when the enqueue failed after the commit. */
  error: string | null
  message_id: string | null
}

export const actionKeys = {
  pending: ['actions', 'pending'] as const,
}

/** Every proposal worth showing: the pending ones, plus the last day's resolutions. */
export function usePendingActions(enabled = true) {
  return useQuery({
    queryKey: actionKeys.pending,
    queryFn: async (): Promise<ActionRow[]> => {
      const body = await api.get<{ actions: ActionRow[] }>('/actions/pending')
      return body.actions
    },
    enabled,
  })
}

/**
 * Confirms a proposal, optionally with an edit.
 *
 * `args` is omitted for a plain confirm, and the body is `{}` rather than `null`:
 * the route's `ConfirmBody` is `deny_unknown_fields` with an optional `args`, so
 * an empty object confirms exactly what was proposed. A `Some` re-parses through
 * the same validator the proposal went through, so an edit can only narrow it —
 * which is why the card's Edit path sends the whole edited object rather than a
 * patch.
 */
export async function confirmAction(id: string, args?: Record<string, unknown>): Promise<ActionRow> {
  const body = await api.post<{ action: ActionRow }>(
    `/actions/${id}/confirm`,
    args === undefined ? {} : { args },
  )
  return body.action
}

/** Dismisses a proposal. Not gated by the plan: a downgraded account can still say no. */
export async function rejectAction(id: string): Promise<ActionRow> {
  const body = await api.post<{ action: ActionRow }>(`/actions/${id}/reject`)
  return body.action
}

/**
 * The two resolutions, and the invalidation each needs.
 *
 * `409 conflict` is a *successful* outcome for the student's intent — the
 * proposal was resolved, just not by this tap — so the caller re-reads the list
 * rather than showing a failure. That is why the invalidation is in `onSettled`
 * and not in `onSuccess`: the second device's tap must end with the card showing
 * what actually happened.
 */
export function useResolveAction() {
  const client = useQueryClient()
  const invalidate = () => client.invalidateQueries({ queryKey: actionKeys.pending })
  const confirm = useMutation({
    mutationFn: ({ id, args }: { id: string; args?: Record<string, unknown> }) =>
      confirmAction(id, args),
    onSettled: invalidate,
  })
  const reject = useMutation({
    mutationFn: (id: string) => rejectAction(id),
    onSettled: invalidate,
  })
  return { confirm, reject }
}
