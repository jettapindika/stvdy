import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'
import type { Proposal } from '@/features/planner/plan'

/**
 * The planner's four routes, and the two shapes they answer in.
 *
 * **Planning is a job, not a route** (A253). The effort estimate is a model call,
 * so `POST /goals/:id/plan` reserves the two credits, writes a proposal row in
 * status `planning`, enqueues the job and answers **202** with `{proposal_id}`;
 * `GET /plans/:id` is how the screen polls. That is why the request returns an id
 * rather than a plan, and why the poll is a query with a `refetchInterval` rather
 * than a mutation that waits — the job outlives the tab that started it, and a
 * reload has to find the proposal still running.
 *
 * **Accepting is a separate act, and it spends nothing.** The two credits were
 * charged at plan time (A264); accept validates the blocks the student adjusted and
 * writes the events, or refuses the whole batch and writes none of them. The
 * preview is a proposal until that call returns.
 */

/** The proposal's lifecycle, as `GET /plans/:id` reports it. */
export type ProposalStatus = 'planning' | 'ready' | 'failed' | 'accepted' | 'superseded'

/** `GET /plans/:id`, field for field. */
export interface ProposalView {
  id: string
  goal_id: string
  status: ProposalStatus
  /** The plan itself, present once the status is `ready` or later. */
  proposal: Proposal | null
  /** A real computed number: the minutes of demand the horizon could not hold. */
  deficit_min: number | null
  warnings: Proposal['warnings'] | null
  prompt_version: string | null
  error: string | null
  created_at: string
  updated_at: string
}

export const planKeys = {
  proposal: (proposalId: string) => ['plans', proposalId] as const,
}

/**
 * One proposal, polled while it is being planned.
 *
 * The interval is off the moment the status settles, so a parked tab stops asking
 * — the same shape `useReport` uses for the same problem.
 */
export function useProposal(proposalId: string) {
  return useQuery({
    queryKey: planKeys.proposal(proposalId),
    queryFn: async ({ signal }) =>
      api.get<{ proposal: ProposalView }>(`/plans/${proposalId}`, { signal }),
    enabled: proposalId.length > 0,
    refetchInterval: (query) =>
      (query.state.data?.proposal.status ?? 'planning') === 'planning' ? 1500 : false,
  })
}

/**
 * `POST /goals/:id/plan` — 202 `{proposal_id}`, two credits reserved.
 *
 * The route takes a `VerifiedUser`: it spends credits and then a model call, and
 * the address must be confirmed first (A43). The free plan's refusal is a 403
 * `plan_required` naming `pro`, which the screen shows as an upgrade prompt rather
 * than as a generic failure — the refusal is structural, not a copy decision
 * (A257).
 */
export async function requestPlan(goalId: string): Promise<string> {
  const body = await api.post<{ proposal_id: string }>(`/goals/${goalId}/plan`)
  return body.proposal_id
}

/** The accept body, as `features/planner/plan.ts`'s `acceptBody` builds it. */
export interface AcceptPayload {
  proposal_id: string
  blocks: {
    starts_at: string
    ends_at: string
    goal_id: string | null
    topic: string
    kind: string
  }[]
}

/**
 * `POST /plans/accept` — the write.
 *
 * A 400 `validation` names the offending block and writes nothing, and a second
 * accept of the same proposal is a 409. Both are refusals the screen shows rather
 * than retries: a preview that accepted a drag and then wrote something else is
 * worse than a refusal.
 */
export async function acceptPlan(payload: AcceptPayload): Promise<string[]> {
  const body = await api.post<{ event_ids: string[] }>('/plans/accept', payload)
  return body.event_ids
}

/** `POST /plan/replan` — 202 `{proposal_id}`, two credits reserved (A265). */
export async function requestReplan(goalId: string): Promise<string> {
  const body = await api.post<{ proposal_id: string }>('/plan/replan', { goal_id: goalId })
  return body.proposal_id
}

/** Invalidates the goal's proposals after a write. */
export function usePlanWrite() {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (goalId: string) => requestPlan(goalId),
    onSuccess: () => {
      void client.invalidateQueries({ queryKey: ['plans'] })
    },
  })
}
