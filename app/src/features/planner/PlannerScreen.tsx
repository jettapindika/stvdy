import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import { useEvents } from '@/api/queries/events'
import { useGoals, type GoalRow } from '@/api/queries/goals'
import { acceptPlan, requestPlan, requestReplan, useProposal } from '@/api/queries/planner'
import { useSettings } from '@/api/queries/settings'
import { useUsage } from '@/api/queries/usage'
import { PageHeader, Section } from '@/components/layout/Section'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { PLANNER_CREDITS, planForPlanner } from '@/features/billing/plans'
import { PrepProgress } from './PrepProgress'
import { ProposalPreview } from './ProposalPreview'
import { WorkloadMeter } from './WorkloadMeter'
import { acceptBody, daysUntil, todayKey, type Interval, type Proposal } from './plan'

/**
 * The planner: the goals, the workload, and the proposal.
 *
 * **The screen asks; it does not schedule.** Every invariant — no overlap with a
 * class, the daily maximum, the study window, the buffer before the deadline — is
 * decided by `stvdy_core::planner`, which reads no clock and no database, and the
 * server is what turns a proposal into rows. What this screen does is show the
 * numbers the server computed and let the student adjust a block, and its own
 * arithmetic (`plan.ts`) is a pure module with its own tests because a second
 * implementation of the server's invariant in a component would be a second
 * schedule.
 *
 * **Nothing is written until Accept.** `POST /goals/:id/plan` reserves the two
 * credits and answers 202 with a proposal id; the proposal is polled, previewed,
 * adjustable, and only `POST /plans/accept` writes events. The cost is on the
 * button before the action runs (honesty gate 7(d)) and it is the matrix's number
 * from `PLANNER_CREDITS`, never a literal.
 *
 * **The free plan's refusal is structural and is shown as an upgrade.** Whether a
 * plan may be generated is `planner != "manual"` (A257), so the locked card is
 * chosen from `GET /usage`'s own `features.planner` and names the plan the matrix
 * grants it on — the same fact the server decides the refusal from.
 *
 * **Rebalance is Power-only**, and it is the balancer (A268): it moves the latest
 * movable block to the least-loaded day, bounded, and it never moves a pinned block
 * or a review-reserved one. A replan keeps the pinned blocks exactly (contract § 6
 * step 7), which is what makes the drag on the preview safe to use.
 */
export function PlannerScreen() {
  const { t } = useTranslation()
  const goals = useGoals()
  const usage = useUsage()
  const settings = useSettings()
  const [selected, setSelected] = useState<string | null>(null)
  const [proposalId, setProposalId] = useState<string | null>(null)
  const [error, setError] = useState<ApiError | null>(null)
  const [saving, setSaving] = useState(false)

  // The instant this view was opened, fixed for the life of the mount: the grid's
  // "today", the current-time line and the days-until figure all come from it, so
  // they cannot disagree with each other or change under the student's hands.
  const [now] = useState(() => new Date())

  const timeZone = usage.data === undefined ? DEFAULT_TIMEZONE : (settings.data?.timezone || DEFAULT_TIMEZONE)
  const rows = goals.data ?? []
  const active = rows.find((goal) => goal.id === selected) ?? rows[0] ?? null

  const plannerMode = usage.data?.features.planner
  const canPlan = plannerMode !== undefined && plannerMode !== 'manual'
  const canRebalance = plannerMode === 'auto+balancer+weakness_blocks'
  const minPlan = planForPlanner()

  const cost = PLANNER_CREDITS.planGeneration

  // The day's accepted study blocks, for the meter. The window is a superset of
  // the local day, so the *server* sends everything the local bucketing might
  // need and `plan.ts` decides which day each row is on — the same split the
  // Today view uses, and for the same reason: a window computed from local
  // instants needs offset arithmetic, and a bug in it silently asks for a window
  // the rows are not in.
  const window = {
    from: new Date(now.getTime() - 36 * 3_600_000).toISOString(),
    to: new Date(now.getTime() + 36 * 3_600_000).toISOString(),
  }
  const events = useEvents(window.from, window.to)
  const today = todayKey(now, timeZone)
  const accepted: Interval[] = (events.data ?? []).filter((event) => event.kind === 'study_block')
  const busy: Interval[] = (events.data ?? [])
    .filter((event) => event.kind !== 'study_block')
    .map((event) => ({ starts_at: event.starts_at, ends_at: event.ends_at }))

  const limitMin = settings.data?.settings.max_study_min_per_day ?? 0
  const studyWindow = {
    start_hour: settings.data?.settings.study_start_hour ?? 8,
    end_hour: settings.data?.settings.study_end_hour ?? 22,
    max_study_min_per_day: limitMin,
  }

  const proposal = useProposal(proposalId ?? '')

  const ask = async (replan: boolean) => {
    if (active === null) return
    setError(null)
    setSaving(true)
    try {
      const id = replan ? await requestReplan(active.id) : await requestPlan(active.id)
      setProposalId(id)
    } catch (thrown) {
      setError(asApiError(thrown))
    } finally {
      setSaving(false)
    }
  }

  const accept = async (blocks: Proposal['blocks']) => {
    if (proposalId === null) return
    setError(null)
    setSaving(true)
    try {
      await acceptPlan({ proposal_id: proposalId, ...acceptBody(blocks) })
      setProposalId(null)
    } catch (thrown) {
      setError(asApiError(thrown))
    } finally {
      setSaving(false)
    }
  }

  const view = proposal.data?.proposal
  const titleOf = (goalId: string) => rows.find((goal) => goal.id === goalId)?.title

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader title={t('nav.planner')} subtitle={t('planner.subtitle')} />

      {/* The spec's `Plan | Tasks` control: Tasks is Phase 9's screen, linked
          rather than copied, so there is one task list and not two. */}
      <nav aria-label={t('planner.views.label')} className="flex flex-wrap gap-2">
        <span className="btn btn-secondary" aria-current="page">
          {t('planner.views.plan')}
        </span>
        <Link to="/tasks" className="btn btn-quiet">
          {t('planner.views.tasks')}
        </Link>
      </nav>

      {error !== null ? (
        <ApiErrorNotice error={error} noun={t('planner.noun')} action={t('actions.retry')} />
      ) : null}

      <Section title={t('planner.goals.title')} count={rows.length > 0 ? rows.length : undefined}>
        <div className="card p-4">
          {goals.isPending ? (
            <LoadingState label={t('planner.goals.loading')} />
          ) : rows.length === 0 ? (
            <EmptyState
              title={t('planner.goals.empty.title')}
              reason={t('planner.goals.empty.reason')}
              padded={false}
              action={
                <Link to="/syllabus" className="link inline-flex min-h-11 items-center">
                  {t('planner.goals.empty.action')}
                </Link>
              }
            />
          ) : (
            <ul className="flex flex-col gap-2">
              {rows.map((goal) => (
                <GoalRowItem
                  key={goal.id}
                  goal={goal}
                  selected={active?.id === goal.id}
                  now={now}
                  timeZone={timeZone}
                  onSelect={() => {
                    setSelected(goal.id)
                    setProposalId(null)
                  }}
                />
              ))}
            </ul>
          )}

          {/* **The import link lives here, not only in the empty state, and that was a
              real defect.** It used to exist *only* inside the `rows.length === 0`
              branch above, and it is the app's **only** link to `/syllabus` — so a
              student with any goal at all could not reach the syllabus import, and
              every student who had used the planner had one. The same class as the
              planner itself having no nav entry: a screen reachable only by typing a
              URL, which no phase's file list owned. Rendering it whenever the list
              renders is what makes the import reachable in both states. */}
          {rows.length > 0 ? (
            <div className="mt-3">
              <Link to="/syllabus" className="link inline-flex min-h-11 items-center">
                {t('planner.goals.importAnother')}
              </Link>
            </div>
          ) : null}
        </div>
      </Section>

      {active !== null ? <PrepProgress goalId={active.id} title={active.title} /> : null}

      {settings.data !== undefined ? (
        <WorkloadMeter
          blocks={accepted}
          dayKey={today}
          timeZone={timeZone}
          limitMin={limitMin}
        />
      ) : null}

      <Section title={t('planner.proposal.title')}>
        {usage.data !== undefined && !canPlan && minPlan !== null ? (
          <LockedFeature
            whatKey="planner.suggest"
            minPlan={minPlan}
            reasonKey="planner.lockedReason"
          />
        ) : (
          <div className="flex flex-col gap-3">
            <div className="flex flex-wrap gap-2">
              <button
                type="button"
                className="btn btn-primary"
                disabled={active === null || saving}
                onClick={() => void ask(false)}
              >
                {cost === null ? t('planner.suggest') : t('planner.suggestWithCost', { count: cost })}
              </button>
              {canRebalance ? (
                <button
                  type="button"
                  className="btn btn-secondary"
                  disabled={active === null || saving}
                  onClick={() => void ask(true)}
                >
                  {t('planner.rebalance')}
                </button>
              ) : (
                <LockedFeature
                  whatKey="planner.rebalance"
                  minPlan="Power"
                  reasonKey="planner.rebalanceReason"
                />
              )}
            </div>

            {proposalId !== null && view === undefined ? (
              <LoadingState label={t('planner.planning')} />
            ) : null}
            {view?.status === 'planning' ? <LoadingState label={t('planner.planning')} /> : null}
            {view?.status === 'failed' ? (
              <p className="note note-danger wrap-anywhere">
                {t('planner.failed', { message: view.error ?? '' })}
              </p>
            ) : null}
            {view?.status === 'superseded' ? (
              <p className="text-small text-muted">{t('planner.superseded')}</p>
            ) : null}

            {view !== null && view !== undefined && view.proposal !== null && view.status === 'ready' ? (
              <ProposalPreview
                // Keyed by the proposal, so a new plan does not inherit the previous
                // one's adjustments: the blocks are the server's, and a stale drag
                // would be a block the student never moved.
                key={view.id}
                proposal={view.proposal}
                now={now}
                timeZone={timeZone}
                settings={studyWindow}
                busy={busy}
                goalTitles={titleOf}
                saving={saving}
                error={error}
                onAccept={(blocks) => void accept(blocks)}
                onDismiss={() => setProposalId(null)}
              />
            ) : null}
          </div>
        )}
      </Section>
    </div>
  )
}

/** One goal: its title, the date it is for, and how long there is. */
function GoalRowItem({
  goal,
  selected,
  now,
  timeZone,
  onSelect,
}: {
  goal: GoalRow
  selected: boolean
  now: Date
  timeZone: string
  onSelect: () => void
}) {
  const { t } = useTranslation()
  const days = daysUntil(goal.target_date, now, timeZone)

  return (
    <li className="flex min-w-0">
      <button
        type="button"
        aria-pressed={selected}
        onClick={onSelect}
        className="flex min-h-11 w-full min-w-0 flex-wrap items-center gap-x-3 gap-y-1 rounded-field px-2 py-2 text-left hover:bg-surface-raised focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring"
      >
        <span className="min-w-0 break-words text-body text-text wrap-anywhere">{goal.title}</span>
        <span className="meta shrink-0">
          {formatInZone(new Date(goal.target_date), timeZone, { dateStyle: 'medium' })}
        </span>
        {/* Days rather than a countdown in hours: the planner works in local days,
            and the exam's own local day is what the student counts to. */}
        <span className="meta shrink-0">
          {days < 0
            ? t('planner.goals.past')
            : t('planner.goals.daysLeft', { count: days })}
        </span>
      </button>
    </li>
  )
}
