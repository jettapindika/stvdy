import { useEffect, useMemo, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import { useEvents, type EventRow } from '@/api/queries/events'
import { useGoals, type GoalRow } from '@/api/queries/goals'
import {
  acceptPlan,
  requestPlan,
  requestReplan,
  useProposal,
  type ProposalStatus,
} from '@/api/queries/planner'
import { useSettings } from '@/api/queries/settings'
import { useUsage } from '@/api/queries/usage'
import { useWorkspaces } from '@/api/queries/workspaces'
import { Section } from '@/components/layout/Section'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { Icon } from '@/components/ui/Icon'
import { formatInZone, localDateKey } from '@/lib/time'
import { PLANNER_CREDITS, planForPlanner } from '@/features/billing/plans'
import { PrepProgress } from '@/features/planner/PrepProgress'
import { ProposalPreview } from '@/features/planner/ProposalPreview'
import { WorkloadMeter } from '@/features/planner/WorkloadMeter'
import {
  acceptBody,
  conflictsWith,
  daysUntil,
  totalMinutes,
  type Interval,
  type ProposedBlock,
} from '@/features/planner/plan'
import { durationLabel } from '@/features/tasks/summary'
import { courseVars } from '@/tokens/tokens'
import { ConnectionsPanel } from './ConnectionsPanel'
import { MonthGrid } from './MonthGrid'
import { TimeGrid, eventCellId } from './TimeGrid'
import { courseHue, localMonth, localWeek, placeEvents } from './calendar'

/**
 * The calendar, as a **panel of the planner**.
 *
 * It is the `Calendar` half of the design's `Calendar | Tasks` pair, and it also
 * carries the plan: every Planner frame draws the suggested study plan *on the
 * calendar* — the banner above the grid, the proposed blocks inside it marked
 * `Suggested · not accepted`, and the prep card beside them. So this panel is
 * `?view=plan` as well as `?view=calendar`, and `PlannerScreen` owns the page
 * heading, the switcher and the footer. The export kept its name because the
 * tests, the accessibility walk and the phase's file list all address it by this
 * one; the role is what changed.
 *
 * **The window is the local one.** `localWeek`/`localMonth` are pure functions of
 * a fixed instant and the student's own IANA zone (never the machine's), so the
 * week starts on the student's Monday and the month holds the student's days —
 * which is the whole reason those functions are unit-tested separately from this
 * screen.
 *
 * **"Today" is the local today, and it is the same instant the window was computed
 * from.** `now` arrives as a prop, captured once by the host, so the heading, the
 * grid, the `Today` cell and the current-time line cannot disagree about which day
 * it is, and a window that changed on every render would be a new query on every
 * render.
 *
 * **Every number on this screen traces to a row** (gate 7). The month cell's count
 * is that day's bucket; the day's list is the rows `GET /events?from=&to=` (or the
 * sync cache) returned for the window; the connections' "last synced" is
 * `calendar_connections.last_synced_at` rendered. There is no "you have 3 hours
 * free" anywhere: nothing in this app computes free time, so nothing here claims
 * it. The one thing this screen says about the future is what the rows say.
 *
 * **Nothing here is generative and nothing spends credits**, so there is no cost
 * on any control — a badge that always reads `0` is noise, and the honesty gate
 * asks for the absence rather than the zero. The plan's own generative action is
 * `PlannerPlan`, which is where the credits and the "Suggested …" label live.
 *
 * The event detail is the one overlay, and it is a `<dialog>`-shaped inline panel
 * rather than a modal: it renders inside the day's own row, `Escape` closes it,
 * and focus returns to the cell that opened it. A trap-free panel is one a
 * keyboard user cannot lose their place in.
 */
export function CalendarScreen({ now, timeZone }: { now: Date; timeZone: string }) {
  const { t } = useTranslation()
  const zone = timeZone
  const usage = useUsage()
  const settings = useSettings()
  const goals = useGoals()
  const workspaces = useWorkspaces()
  const [selectedGoal, setSelectedGoal] = useState<string | null>(null)
  const [proposalId, setProposalId] = useState<string | null>(null)
  const [saving, setSaving] = useState(false)
  const [error, setError] = useState<ApiError | null>(null)
  const [refusal, setRefusal] = useState<string | null>(null)

  // The proposed blocks and the student's keep flags, held here rather than in
  // `ProposalPreview` because the banner's `Accept all` / `Accept selected` are
  // what act on them. One copy, so the banner and the grid cannot disagree about
  // what will be written.
  const [blocks, setBlocks] = useState<ProposedBlock[]>([])
  const [kept, setKept] = useState<boolean[]>([])
  const [loadedProposal, setLoadedProposal] = useState<string | null>(null)

  const goalRows = goals.data ?? []
  const active = goalRows.find((goal) => goal.id === selectedGoal) ?? goalRows[0] ?? null

  const plannerMode = usage.data?.features.planner
  const canPlan = plannerMode !== undefined && plannerMode !== 'manual'
  const canRebalance = plannerMode === 'auto+balancer+weakness_blocks'
  const minPlan = planForPlanner()
  const cost = PLANNER_CREDITS.planGeneration

  const proposal = useProposal(proposalId ?? '')
  const plan = proposal.data?.proposal ?? null

  // A new proposal replaces the block list outright: the blocks are the server's,
  // and keeping a stale drag across two proposals would be a block the student
  // never moved. Guarded on the id so the effect runs once per proposal rather
  // than on every poll of the same one.
  useEffect(() => {
    if (plan === null || plan.status !== 'ready' || plan.proposal === null) return
    if (loadedProposal === plan.id) return
    setLoadedProposal(plan.id)
    setBlocks(plan.proposal.blocks)
    setKept(plan.proposal.blocks.map(() => true))
    setRefusal(null)
  }, [plan, loadedProposal])

  // The window the meter and the drag validation read: a superset of the local
  // day, so the *server* sends everything the local bucketing might need and
  // `plan.ts` decides which day each row is on. A window computed from local
  // instants needs offset arithmetic, and a bug in it silently asks for a window
  // the rows are not in.
  const todayWindow = {
    from: new Date(now.getTime() - 36 * 3_600_000).toISOString(),
    to: new Date(now.getTime() + 36 * 3_600_000).toISOString(),
  }
  const todayEvents = useEvents(todayWindow.from, todayWindow.to)

  /**
   * The proposal's own range, when there is one.
   *
   * **The busy set the drag is judged against has to cover the blocks being
   * dragged.** A proposal for an exam three weeks out places blocks three weeks
   * out, and a `busy` list built from a 36-hour window around today is empty for
   * every one of them — so a block dropped on top of a lecture would be accepted
   * locally and refused by the server after a round trip, which is exactly what
   * the local check exists to avoid. So the range is the blocks' own, widened by
   * an hour on each side, and the events in it are what the conflicts count and
   * the refusal both read.
   */
  const proposalRange =
    blocks.length === 0
      ? null
      : {
          from: new Date(
            Math.min(...blocks.map((block) => Date.parse(block.starts_at))) - 3_600_000,
          ).toISOString(),
          to: new Date(
            Math.max(...blocks.map((block) => Date.parse(block.ends_at))) + 3_600_000,
          ).toISOString(),
        }
  const planEvents = useEvents(
    proposalRange?.from ?? todayWindow.from,
    proposalRange?.to ?? todayWindow.to,
  )

  const todayKey = localDateKey(now, zone)
  const accepted: Interval[] = (todayEvents.data ?? []).filter(
    (event) => event.kind === 'study_block',
  )
  const busy: Interval[] = (planEvents.data ?? [])
    .filter((event) => event.kind !== 'study_block')
    .map((event) => ({ starts_at: event.starts_at, ends_at: event.ends_at }))

  const limitMin = settings.data?.settings.max_study_min_per_day ?? 0
  const studyWindow = {
    start_hour: settings.data?.settings.study_start_hour ?? 8,
    end_hour: settings.data?.settings.study_end_hour ?? 22,
    max_study_min_per_day: limitMin,
  }

  const ask = async (replan: boolean) => {
    if (active === null) return
    setError(null)
    setSaving(true)
    try {
      setProposalId(replan ? await requestReplan(active.id) : await requestPlan(active.id))
    } catch (thrown) {
      setError(asApiError(thrown))
    } finally {
      setSaving(false)
    }
  }

  const accept = async (only: 'all' | 'selected') => {
    if (proposalId === null) return
    const chosen = only === 'all' ? blocks : blocks.filter((_block, at) => kept[at] ?? true)
    if (chosen.length === 0) {
      // A real refusal rather than a silent no-op: `POST /plans/accept` with an
      // empty list would write nothing and answer 200, which reads as success.
      setRefusal('empty')
      return
    }
    setError(null)
    setSaving(true)
    try {
      await acceptPlan({ proposal_id: proposalId, ...acceptBody(chosen) })
      setProposalId(null)
      setBlocks([])
      setKept([])
      setLoadedProposal(null)
    } catch (thrown) {
      setError(asApiError(thrown))
    } finally {
      setSaving(false)
    }
  }


  const [view, setView] = useState<'week' | 'month'>('week')
  const [selected, setSelected] = useState<string | null>(null)
  // The cell that opened the detail, so `Escape` can hand focus back to it. A
  // detail that closed into `document.body` would leave the keyboard user at the
  // top of the page with no idea where they were.
  const opener = useRef<string | null>(null)

  const week = useMemo(() => localWeek(now, zone), [now, zone])
  const month = useMemo(() => localMonth(now, zone), [now, zone])
  const monthKey = todayKey.slice(0, 7)

  const days = view === 'week' ? week : month
  const first = days[0]?.key ?? todayKey
  const last = days[days.length - 1]?.key ?? todayKey
  // A **superset** of the local window, deliberately: the exact local edges need
  // the zone's offset arithmetic here, and a bug in it would silently ask for a
  // window the rows are not in — the one failure this screen cannot see, because
  // it renders whatever comes back. So the window is the grid's own days widened
  // by a day on each side (no zone is more than 14 hours from UTC), and which day
  // a row is *on* is decided by `placeEvents`, in the student's zone. This is
  // `TodayScreen`'s rule and the same reasoning.
  const from = new Date(Date.parse(`${first}T00:00:00Z`) - 24 * 60 * 60 * 1000).toISOString()
  const to = new Date(Date.parse(`${last}T23:59:59Z`) + 24 * 60 * 60 * 1000).toISOString()
  const events = useEvents(from, to)

  const buckets = useMemo(
    () => placeEvents(events.data ?? [], days, zone),
    [events.data, days, zone],
  )

  const workspaceNames = useMemo(() => {
    const names: Record<string, string> = {}
    for (const workspace of workspaces.data ?? []) names[workspace.id] = workspace.name
    return names
  }, [workspaces.data])

  /**
   * Each workspace's server-assigned course index, so a course wears the same hue here
   * as it does on its card and in the sidebar.
   *
   * The grid used to derive the hue from the workspace id (`courseHue`'s hash), which
   * was the only source available before `workspaces.color` existed — so the same course
   * could show one colour on the calendar and another on the card that links to it. This
   * map is built from the list the screen already fetches; the hash survives inside
   * `courseHue` as the fallback for an event whose workspace has not loaded.
   */
  const workspaceColors = useMemo(() => {
    const colors = new Map<string, number>()
    for (const workspace of workspaces.data ?? []) colors.set(workspace.id, workspace.color)
    return colors
  }, [workspaces.data])

  useEffect(() => {
    if (selected === null) return undefined
    const onKey = (event: KeyboardEvent) => {
      if (event.key !== 'Escape') return
      setSelected(null)
      const id = opener.current
      if (id !== null) document.getElementById(eventCellId(id))?.focus()
    }
    document.addEventListener('keydown', onKey)
    return () => {
      document.removeEventListener('keydown', onKey)
    }
  }, [selected])

  const total = events.data?.length ?? 0

  // The next goal still ahead of today, soonest first. A goal is the app's own
  // record of an exam or a deadline, so this is a row rather than a guess, and an
  // empty list renders nothing rather than a sentence about an exam nobody
  // recorded.
  // `target_date` is a `timestamptz`, so it is an instant and comparing it to a date
  // key with `>=` is a **string** comparison between two different shapes
  // (`"2026-11-16T09:00:00Z" >= "2026-11-16"` is true, but so is `"2026-11-16T00:00:00Z"
  // >= "2026-11-17"` being *false* for the wrong reason). The question this answers is
  // "is the exam still ahead in the reader's own calendar", so it is asked in local
  // days: the same `localDateKey` the grid and the Today view use for the same field.
  const next =
    goalRows
      .filter((goal) => localDateKey(new Date(goal.target_date), zone) >= todayKey)
      .sort((left, right) => (left.target_date < right.target_date ? -1 : 1))[0] ?? null
  const nextCourse = next === null ? undefined : workspaceNames[next.workspace_id]

  return (
    <div className="flex w-full min-w-0 flex-col gap-6">
      {error !== null ? (
        <ApiErrorNotice error={error} noun={t('planner.noun')} action={t('actions.retry')} />
      ) : null}

      {/* The suggested plan, at the top of the calendar, which is where the design
          draws it: a `Suggested …` banner over the grid the blocks will land in. */}
      <Section title={t('planner.proposal.title')}>
        <PlanBanner
          goal={active}
          blocks={blocks}
          kept={kept}
          busy={busy}
          saving={saving}
          canPlan={canPlan}
          canRebalance={canRebalance}
          minPlan={minPlan}
          cost={cost}
          status={plan?.status ?? null}
          failed={plan?.error ?? null}
          superseded={plan?.status === 'superseded'}
          deficitMin={plan?.proposal?.deficit_min ?? null}
          refusal={refusal}
          onSuggest={() => void ask(false)}
          onRebalance={() => void ask(true)}
          onAccept={(only) => void accept(only)}
          onDismiss={() => {
            setProposalId(null)
            setBlocks([])
            setKept([])
            setLoadedProposal(null)
            setRefusal(null)
          }}
        />
      </Section>

      <div className="grid min-w-0 gap-6 lg:grid-cols-[minmax(0,2fr)_minmax(0,1fr)]">
      {/* Wrapped rather than classed: `Section` is frozen and its root is a flex
          column, so as a grid item its automatic minimum could exceed the track
          the `minmax(0, …)` declares. `min-w-0` on the wrapper is the permission
          that keeps a wide descendant inside its own column. */}
      <div className="flex min-w-0 flex-col">
      <Section title={view === 'week' ? t('calendar.week.title') : t('calendar.month.title')}>
        {/* The controls and the legend sit above the grid, which is where the
            design draws them: the period on the left, the scope on the right, and
            a key under both that names what each mark in the grid means. */}
        <div className="flex flex-col gap-3">
          <div className="flex flex-wrap items-center justify-between gap-3">
            <div
              role="group"
              aria-label={t('calendar.view.title')}
              className="flex flex-wrap items-center gap-3"
            >
              <button
                type="button"
                className="btn btn-secondary"
                aria-pressed={view === 'week'}
                onClick={() => {
                  setView('week')
                  setSelected(null)
                }}
              >
                {t('calendar.view.week')}
              </button>
              <button
                type="button"
                className="btn btn-secondary"
                aria-pressed={view === 'month'}
                onClick={() => {
                  setView('month')
                  setSelected(null)
                }}
              >
                {t('calendar.view.month')}
              </button>
            </div>
            {/* The count is the rows in the window, rendered as the number it is.
                A pending read says nothing rather than `0`. */}
            {events.data !== undefined && (
              <p className="meta">{t('calendar.eventCount', { count: total })}</p>
            )}
          </div>

          <ul className="flex flex-wrap items-center gap-x-6 gap-y-2">
            <li className="flex min-w-0 items-center gap-2">
              <Icon name="lock" size={16} className="text-muted" />
              <span className="meta min-w-0 break-words">{t('calendar.legend.imported')}</span>
            </li>
            <li className="flex min-w-0 items-center gap-2">
              <span
                aria-hidden="true"
                className="h-4 w-4 shrink-0 rounded border border-success bg-success-tint"
              />
              <span className="meta min-w-0 break-words">{t('calendar.legend.scheduled')}</span>
            </li>
            <li className="flex min-w-0 items-center gap-2">
              <span
                aria-hidden="true"
                className="h-4 w-4 shrink-0 rounded border border-primary"
              />
              <span className="meta min-w-0 break-words text-primary-text">
                {t('honesty.suggested', { what: t('planner.proposal.what') })}
              </span>
            </li>
          </ul>
        </div>

        {events.isPending && <LoadingState label={t('calendar.loading')} />}
        {events.data !== undefined && total === 0 && (
          <EmptyState title={t('calendar.empty.title')} reason={t('calendar.empty.reason')} />
        )}
        {events.data !== undefined &&
          total > 0 &&
          (view === 'week' ? (
            <TimeGrid
              buckets={buckets}
              zone={zone}
              todayKey={todayKey}
              now={now}
              selected={selected}
              onSelect={(id) => {
                opener.current = id
                setSelected((current) => (current === id ? null : id))
              }}
              workspaceNames={workspaceNames}
              courseColors={workspaceColors}
              detail={(event) => (
                <EventDetail event={event} zone={zone} courseColors={workspaceColors} />
              )}
            />
          ) : (
            <MonthGrid
              buckets={buckets}
              zone={zone}
              todayKey={todayKey}
              month={monthKey}
              onOpenWeek={() => {
                setView('week')
                setSelected(null)
              }}
            />
          ))}
      </Section>
      </div>

      {/* The exam context, beside the grid on a desktop and below it on a phone:
          the prep card for the selected goal, the day's planned load, and the
          exams ahead. It is the design's right-hand column. */}
      <aside className="flex min-w-0 flex-col gap-6">
        {active !== null ? (
          <PrepProgress goalId={active.id} title={active.title} />
        ) : null}
        {settings.data !== undefined ? (
          <WorkloadMeter blocks={accepted} dayKey={todayKey} timeZone={zone} limitMin={limitMin} />
        ) : null}

        {/* Which exam the plan is for, and the exams ahead. One list rather than
            two: the frame puts "Upcoming exams" in this column and the plan banner
            names one goal, so a separate goal chooser would print the same title
            twice on one screen. Each row is the chooser — `aria-pressed` marks the
            active one and the banner names it — and the import link renders in
            both states, because a student with goals still needs the second
            import. */}
        <Section
          title={t('planner.goals.title')}
          count={goalRows.length > 0 ? goalRows.length : undefined}
        >
          <div className="card p-4">
            {goals.isPending ? (
              <LoadingState label={t('planner.goals.loading')} />
            ) : goalRows.length === 0 ? (
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
                {goalRows.map((goal) => (
                  <GoalRowItem
                    key={goal.id}
                    goal={goal}
                    course={workspaceNames[goal.workspace_id]}
                    selected={active?.id === goal.id}
                    now={now}
                    timeZone={zone}
                    onSelect={() => {
                      setSelectedGoal(goal.id)
                      setProposalId(null)
                      setBlocks([])
                      setKept([])
                      setLoadedProposal(null)
                    }}
                  />
                ))}
              </ul>
            )}
            {goalRows.length > 0 ? (
              <div className="mt-3">
                <Link to="/syllabus" className="link inline-flex min-h-11 items-center">
                  {t('planner.goals.importAnother')}
                </Link>
              </div>
            ) : null}
          </div>
        </Section>
      </aside>
      </div>

      {/* The next exam, when the account has a goal ahead of today. The design's
          sentence points at the plan, and the plan is this screen — so the control
          scrolls to it rather than pretending to navigate somewhere else. No goal
          renders nothing at all: a note about an exam the app does not know about
          would be the invention this product forbids. */}
      {next !== null ? (
        <div className="raised flex flex-wrap items-center gap-3 border border-border p-4">
          <Icon name="calendar" size={20} className="text-primary-text" />
          <p className="min-w-0 flex-1 break-words text-label text-muted">
            {t('planner.nextExam', {
              course: nextCourse ?? next.title,
              // `target_date` is a `timestamptz` (`migrations/0012`), so it is an
              // instant and `new Date()` reads it directly. Appending `T12:00:00Z`
              // — which a bare `YYYY-MM-DD` would need — produced
              // `2026-11-11T09:00:00ZT12:00:00Z`, an **Invalid Date**, and
              // `Intl.DateTimeFormat.format` throws a `RangeError` on one: the whole
              // screen fell to the router's error boundary. The reader's zone is what
              // decides which day this is, which is the same rule the goals list
              // below applies to the same field.
              date: formatInZone(new Date(next.target_date), zone, {
                day: 'numeric',
                month: 'short',
              }),
            })}
          </p>
          <a href="#plan" className="btn btn-secondary">
            {t('planner.nextExamAction')}
          </a>
        </div>
      ) : null}

      {/* The proposal's blocks, when there is one. It is a section of its own
          rather than blocks drawn inside the grid above, and that is a deliberate
          difference from the frame: the frame's suggested blocks sit in the week
          grid, but this grid renders *events* — rows that exist — and a proposal
          is not a row until it is accepted. Drawing one as the other is how a
          student comes to believe a suggestion was already saved, which is the
          exact lie the `Suggested …` label exists to prevent. */}
      {plan !== null && plan.proposal !== null && plan.status === 'ready' && active !== null ? (
        <Section title={t('honesty.suggested', { what: t('planner.proposal.what') })}>
          <ProposalPreview
            proposal={plan.proposal}
            now={now}
            timeZone={zone}
            settings={studyWindow}
            busy={busy}
            goalTitles={(goalId) => goalRows.find((goal) => goal.id === goalId)?.title}
            blocks={blocks}
            onBlocks={setBlocks}
            kept={kept}
            onKept={setKept}
          />
        </Section>
      ) : null}

      {/* Imported classes are not editable here, and the sentence says which half
          of the grid that applies to (A198/A240). It is the design's own
          explanation line, kept because a student who taps an imported class and
          finds no editor needs to know why before they look for one. */}
      <p className="flex items-start gap-2 text-small text-muted">
        <Icon name="lock" size={16} className="mt-0.5 shrink-0" />
        <span className="min-w-0 break-words">{t('planner.readOnlyNote')}</span>
      </p>

      <ConnectionsPanel />
    </div>
  )
}

/**
 * One event, in full: what it is, when it is, and whether it can be edited.
 *
 * The read-only sentence is **words**, not only the lock icon, and it says which
 * of the two reasons applies: a Google or ICS row came from somewhere else and
 * this app will not let it be edited here, and the next sync would revert an edit
 * that landed anyway (A198/A240). "Read-only" alone would leave the student
 * wondering whether it was a permission problem on their account.
 *
 * Nothing here edits, deletes or creates. An event editor is the Today view's
 * capture and the assistant's confirmed actions, and a second write path for
 * `events` on this panel would be the thing A198 exists to prevent.
 */
function EventDetail({
  event,
  zone,
  courseColors,
}: {
  event: EventRow
  zone: string
  /** The server's index per workspace, so the hue matches the course's own card. */
  courseColors: ReadonlyMap<string, number>
}) {
  const { t } = useTranslation()
  const hue = courseHue(event.workspace_id, courseColors)
  const external = event.source !== 'app'

  return (
    <div className="panel flex flex-col gap-2 p-4" style={hue === null ? undefined : courseVars(hue)}>
      <div className="flex flex-wrap items-center gap-2">
        {hue === null ? null : <span className="course-marker h-6" aria-hidden="true" />}
        <h3 className="min-w-0 break-words">{event.title}</h3>
        {external && (
          <span className="chip flex items-center gap-1">
            <Icon name="lock" size={16} />
            {t('calendar.event.readOnly')}
          </span>
        )}
      </div>
      <p className="text-small text-muted">
        {t('calendar.event.when', {
          start: formatInZone(new Date(event.starts_at), zone, {
            dateStyle: 'medium',
            timeStyle: 'short',
          }),
          end: formatInZone(new Date(event.ends_at), zone, {
            dateStyle: 'medium',
            timeStyle: 'short',
          }),
        })}
      </p>
      <p className="text-small text-muted">
        {t('calendar.event.kind', { kind: t(`calendar.kind.${event.kind}`) })}
      </p>
      {external && (
        <p className="max-w-prose text-small text-subtle">
          {event.source === 'google'
            ? t('calendar.event.readOnlyGoogle')
            : t('calendar.event.readOnlyIcs')}
        </p>
      )}
    </div>
  )
}

/**
 * The suggested plan's banner: what it is, how big it is, and the three actions.
 *
 * **Nothing here is applied until a button is pressed**, and the label says so:
 * `Suggested study plan for …` and `Nothing is added until you confirm.` are the
 * honesty gate's copy (7e), and the banner is the last thing between a proposal and
 * the write. `Accept all` writes every block; `Accept selected` writes the checked
 * ones; `Dismiss` writes none. An `Accept selected` with nothing selected is
 * refused with a sentence rather than sent as an empty batch, because
 * `POST /plans/accept` answers 200 to an empty list and a 200 reads as success.
 *
 * **The two figures are computed from the blocks, not from the frame.** The count
 * is `blocks.length` and the duration is `totalMinutes(blocks)` — the same
 * `blockMinutes` the workload meter uses, so the banner and the meter cannot
 * disagree about how long a session is. The conflict count is
 * `conflictsWith(blocks, busy)` over the proposal's own window: a count of the
 * blocks in hand that share an instant with something already scheduled, which is
 * a measurement rather than a claim that the schedule is sound.
 *
 * **The generative action carries its cost** before it runs (gate 7d), from
 * `PLANNER_CREDITS` rather than a literal, and the free plan's refusal is the
 * matrix's own `features.planner` rather than a guess.
 */
function PlanBanner({
  goal,
  blocks,
  kept,
  busy,
  saving,
  canPlan,
  canRebalance,
  minPlan,
  cost,
  status,
  failed,
  superseded,
  deficitMin,
  refusal,
  onSuggest,
  onRebalance,
  onAccept,
  onDismiss,
}: {
  goal: GoalRow | null
  blocks: ProposedBlock[]
  kept: boolean[]
  busy: Interval[]
  saving: boolean
  canPlan: boolean
  canRebalance: boolean
  minPlan: 'Pro' | 'Power' | null
  cost: number | null
  status: ProposalStatus | null
  failed: string | null
  superseded: boolean
  deficitMin: number | null
  refusal: string | null
  onSuggest: () => void
  onRebalance: () => void
  onAccept: (only: 'all' | 'selected') => void
  onDismiss: () => void
}) {
  const { t } = useTranslation()
  const conflicts = conflictsWith(blocks, busy)
  const selectedCount = blocks.filter((_block, at) => kept[at] ?? true).length
  const planning = status === 'planning'

  return (
    <div className="raised flex flex-col gap-4 border border-border p-5">
      <div className="flex flex-col gap-2">
        {/* The label is the honesty gate's, and it is never `Applied`: a proposal
            the student has not confirmed is a suggestion. */}
        <p className="eyebrow-primary">{t('honesty.suggested', { what: t('planner.proposal.what') })}</p>
        <h3 className="min-w-0 break-words">
          {goal === null
            ? t('planner.proposal.title')
            : t('planner.proposal.banner', { goal: goal.title })}
        </h3>
        <p className="min-w-0 break-words text-small text-muted">
          {t('honesty.nothingAddedYet')}
        </p>

        {blocks.length > 0 ? (
          <p className="meta">
            {t('planner.proposal.context', {
              count: blocks.length,
              duration: durationLabel(t, totalMinutes(blocks)),
            })}
          </p>
        ) : null}

        {/* A real measurement over the rows in hand. Zero is a real count here:
            the app did look at every block it has. */}
        {blocks.length > 0 ? (
          <p className="meta">{t('planner.conflicts', { count: conflicts })}</p>
        ) : null}

        {/* The deficit is a real computed number, so it is shown as one. A plan
            that could not fit everything says how much it could not fit rather
            than looking complete. */}
        {deficitMin !== null && deficitMin > 0 ? (
          <p className="note note-warning wrap-anywhere">
            {t('planner.deficit', { minutes: deficitMin })}
          </p>
        ) : null}
      </div>

      {refusal === 'empty' ? (
        <p className="note note-warning wrap-anywhere">{t('planner.proposal.noneKept')}</p>
      ) : null}

      {failed !== null ? (
        <p className="note note-danger wrap-anywhere">
          {t('planner.failed', { message: failed })}
        </p>
      ) : null}
      {superseded ? <p className="text-small text-muted">{t('planner.superseded')}</p> : null}
      {planning ? <LoadingState label={t('planner.planning')} /> : null}

      <div className="flex flex-wrap gap-2">
        {!canPlan && minPlan !== null ? (
          <LockedFeature
            whatKey="planner.suggest"
            minPlan={minPlan}
            reasonKey="planner.lockedReason"
          />
        ) : (
          <button
            type="button"
            className="btn btn-primary"
            disabled={goal === null || saving}
            onClick={onSuggest}
          >
            {cost === null
              ? t('planner.suggest')
              : t('planner.suggestWithCost', { count: cost })}
          </button>
        )}

        {canRebalance ? (
          <button
            type="button"
            className="btn btn-secondary"
            disabled={goal === null || saving}
            onClick={onRebalance}
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

      {blocks.length > 0 ? (
        <div className="flex flex-col gap-2">
          <p className="meta">
            {t('planner.proposal.selectedCount', { count: selectedCount })}
          </p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-primary"
              disabled={saving}
              onClick={() => onAccept('all')}
            >
              {t('planner.proposal.accept')}
            </button>
            <button
              type="button"
              className="btn btn-secondary"
              disabled={saving || selectedCount === 0}
              onClick={() => onAccept('selected')}
            >
              {t('planner.proposal.acceptSelected')}
            </button>
            <button type="button" className="btn btn-quiet" onClick={onDismiss}>
              {t('actions.dismiss')}
            </button>
          </div>
          {saving ? <LoadingState label={t('planner.proposal.saving')} /> : null}
        </div>
      ) : null}
    </div>
  )
}

/** One goal: its title, the date it is for, and how long there is. */
function GoalRowItem({
  goal,
  course,
  selected,
  now,
  timeZone,
  onSelect,
}: {
  goal: GoalRow
  /** `BIO 201 · Cell biology`, or undefined when the workspace has no code. */
  course: string | undefined
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
        <span className="flex min-w-0 flex-col gap-0.5">
          {course === undefined ? null : (
            <span className="meta min-w-0 break-words text-primary-text">{course}</span>
          )}
          <span className="min-w-0 break-words text-body text-text wrap-anywhere">
            {goal.title}
          </span>
        </span>
        <span className="meta shrink-0">
          {formatInZone(new Date(goal.target_date), timeZone, { dateStyle: 'medium' })}
        </span>
        {/* Days rather than a countdown in hours: the planner works in local days,
            and the exam's own local day is what the student counts to. */}
        <span className="meta shrink-0">
          {days < 0 ? t('planner.goals.past') : t('planner.goals.daysLeft', { count: days })}
        </span>
      </button>
    </li>
  )
}
