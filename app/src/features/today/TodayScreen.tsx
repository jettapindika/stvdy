import { useCallback, useEffect, useMemo, useRef, useState, type ReactNode } from 'react'
import type { TFunction } from 'i18next'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { useSyncStatus } from '@/api/queries/sync'
import { CreditMeter } from '@/features/billing/CreditMeter'
import { PageFooter, PageHeader, Section, WidgetHeading } from '@/components/layout/Section'
import { displayNameOf } from '@/components/layout/TopBar'
import { Icon } from '@/components/ui/Icon'
import { EmptyState, ErrorState, LoadingState } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE, formatInZone, localDateKey, zoneOffsetLabel } from '@/lib/time'
import { COURSE_NAMES, COURSE_VARS } from '@/tokens/tokens'
import { FocusTimer } from './FocusTimer'
import { QuickCapture } from './QuickCapture'
import { dayOffset, todayBuckets, type RecentDay, type TodayEntry } from './day'
import { useTodayData, type CourseRef } from './useTodayData'

/**
 * Today, the default home.
 *
 * Five things this screen is built to get right, and each is a gate rather than a
 * preference.
 *
 * **Every number traces to a row** (gate 7). "N due for review" is the number of
 * cards in the review queue, the minutes are the sum of today's
 * `study_sessions.duration_min`, the streak is consecutive local days that hold a
 * review, an attempt or a session, and the checklist's counts are live
 * workspaces, sources and decks. Not one is a placeholder, and the buckets are
 * computed by `day.ts`, which is a pure function of the rows the device already
 * holds — which is also why this screen works with the network down (A205).
 *
 * **Nothing is invented while loading.** A pending read renders a loading state,
 * not zeroes: a screen that showed "0 due" while the read was in flight would be
 * telling the student they have nothing to do.
 *
 * **A figure that is absent is an empty state, never a zero.** The streak is the
 * one this screen states in words: a student with no streak sees what a day
 * counts and where to start, not "0 day streak" — the zero would be the app using
 * a number to say "you have nothing", and the honest form of that is a sentence.
 *
 * **The offline state is a screen, not a hidden one.** The band above the content
 * names the cause and what still works, and every control stays where it is: a
 * student on a train still has today's tasks, today's events and today's reviews,
 * and hiding the review button would take away the one action that works offline.
 * The one thing that genuinely needs the network — the assistant — says so under
 * its own control.
 */
export function TodayScreen() {
  const { t } = useTranslation()
  const session = useSession()
  const timeZone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

  // The real wall-clock instant, fixed for the mount's lifetime.
  const [realNow] = useState(() => new Date())
  const realTodayKey = localDateKey(realNow, timeZone)

  // The day the screen is showing. Defaults to now; the date strip changes it.
  const [selectedDate, setSelectedDate] = useState<Date | null>(null)
  const now = selectedDate ?? realNow
  const selectedKey = localDateKey(now, timeZone)

  // The data window covers the strip's full range plus margin for the week
  // deadlines widget (7 days ahead of the selected day). This is a superset
  // that lets the user tap any strip day without a new fetch.
  const window = {
    from: new Date(realNow.getTime() - 4 * 24 * 60 * 60 * 1000).toISOString(),
    to: new Date(realNow.getTime() + 11 * 24 * 60 * 60 * 1000).toISOString(),
  }
  const data = useTodayData(window)
  const { report } = useSyncStatus()

  const buckets = todayBuckets({
    now,
    timeZone,
    tasks: data.tasks,
    events: data.events,
    sessions: data.sessions,
    dueCards: data.dueCards,
    activity: data.activity,
    setup: data.setup,
  })

  const isNewUser =
    buckets.setup.workspaces === 0 && buckets.setup.sources === 0 && buckets.setup.decks === 0
  const offline = report?.state === 'offline'

  // The greeting uses the real clock, not the selected day: greeting a student
  // with "Good morning" because they tapped a past morning would be a lie.
  const localHour = Number(
    new Intl.DateTimeFormat('en-GB', { hour: 'numeric', hour12: false, timeZone }).format(realNow),
  )
  const period = localHour < 12 ? 'morning' : localHour < 18 ? 'afternoon' : 'evening'
  const name = session.status === 'signed-in' ? displayNameOf(session.user.email) : ''

  // When every query has either settled or errored and at least one errored,
  // show the error state instead of the empty-looking "nothing to study" screen
  // that a network failure would otherwise produce (N2).
  if (data.failed && !data.loading) {
    return (
      <div className="flex w-full flex-col gap-6 px-5 py-6 md:px-[32px] md:py-8">
        <PageHeader
          eyebrow={formatInZone(now, timeZone, {
            weekday: 'long',
            day: 'numeric',
            month: 'long',
            year: 'numeric',
          })}
          title={t('nav.today')}
          subtitle={name.length > 0 ? t(`today.greeting.${period}`, { name }) : t('today.subtitle')}
        />
        <ErrorState
          title={t('today.failed.title')}
          reason={t('today.failed.reason')}
          action={
            <button
              type="button"
              className="btn btn-primary"
              onClick={() => globalThis.location.reload()}
            >
              {t('states.reload')}
            </button>
          }
        />
      </div>
    )
  }

  // The course of each row, so a timeline entry or a deadline can be tagged with
  // the course it belongs to. Built once: a lookup per row would be a scan of the
  // course list per row.
  const courses = new Map(data.courses.map((course) => [course.id, course]))

  const rows = buckets.today.map((entry) =>
    timelineRow(
      entry,
      courses.get(
        (entry.kind === 'task' ? entry.task.workspace_id : entry.event.workspace_id) ?? '',
      ),
      t,
      now,
    ),
  )

  return (
    <div className="flex w-full flex-col gap-6 px-5 py-6 md:px-[32px] md:py-8">
      {offline ? (
        <div className="note flex flex-wrap items-center gap-3" role="status">
          <Icon name="alert" className="shrink-0 text-warning" />
          <p className="min-w-0 flex-1 text-label">
            {t('states.offline')} — {t('states.offlineReason')}
          </p>
          <span className="meta text-warning">{t('sync.state.offline')}</span>
        </div>
      ) : null}

      <PageHeader
        eyebrow={formatInZone(now, timeZone, {
          weekday: 'long',
          day: 'numeric',
          month: 'long',
          year: 'numeric',
        })}
        title={t('nav.today')}
        subtitle={name.length > 0 ? t(`today.greeting.${period}`, { name }) : t('today.subtitle')}
        action={
          <DatePickerPopover
            now={now}
            selectedKey={selectedKey}
            realTodayKey={realTodayKey}
            onSelect={setSelectedDate}
          />
        }
      />
      <div className="account-timezone-row flex flex-wrap items-center justify-between gap-3 md:hidden">
        <CreditMeter />
        <span className="meta">{timeZone.toUpperCase()}</span>
      </div>

      {isNewUser ? (
        <Section title={t('today.setup.title')}>
          <div className="card flex flex-col gap-3 p-4">
            <ul className="flex flex-col gap-1">
              <li className="text-body">
                {t('today.setup.workspaces', { count: buckets.setup.workspaces })}
              </li>
              <li className="text-body">
                {t('today.setup.sources', { count: buckets.setup.sources })}
              </li>
              <li className="text-body">{t('today.setup.decks', { count: buckets.setup.decks })}</li>
            </ul>
            <p className="text-small text-muted">{t('today.setup.hint')}</p>
            <Link to="/workspaces" className="link inline-flex min-h-11 items-center self-start">
              {t('today.setup.start')}
            </Link>
          </div>
        </Section>
      ) : null}

      {/* One grid, six blocks, and the desktop frame's two columns come from the
          explicit placement below. The phone's order is the frame's own — the
          review first, the capture second, the schedule after it — and it is
          `order-*` because the desktop bands group blocks the phone does not:
          `order` decides nothing once every block names its row and column. */}
      <div className="grid grid-cols-1 gap-6 lg:grid-cols-[7fr_3fr]">
        {/* --- Priority ------------------------------------------------------- */}
        <div className="order-1 flex flex-col [&>*:first-child]:flex-1 lg:col-start-1 lg:row-start-1">
          {/* The frame's priority block: the count and its context on the left, the
              one action on the right with the queue's name under it, the two
              vertically centred against each other on a desktop and stacked on a
              phone — which is the frame's own phone arrangement. The desktop row
              **wraps** rather than shrinks: at a 32px root the count and the button
              cannot share one line, and a pair that cannot wrap is a pair that
              scrolls the page sideways. */}
          <div className="raised flex flex-col gap-4 p-6 lg:flex-row lg:flex-wrap lg:items-center lg:justify-between lg:gap-6">
            {data.loading ? (
              <LoadingState label={t('today.dueCards.loading')} />
            ) : buckets.dueCards > 0 && data.reviewDeckId !== null ? (
              <>
                {/* `min-w-[min(14rem,100%)]`, not `min-w-0` and not a bare `min-w-56`.
                    The block is `[text | action]` and the action column is 14rem, so
                    `min-w-0` let the text column be squeezed to **75px** at a 32px root
                    and clipped the deck line (measured: 140px of scroll in a 75px box,
                    both themes). A bare rem floor fixes that at 1365px and breaks below
                    ~1320px, where 14rem (448px) is wider than the card's own content box
                    and the column overflows its border box — the defect `QuizPanel.tsx`
                    and `DevicesScreen.tsx` both record. The cap makes the floor collapse
                    to the container's own width where there is less room than the floor,
                    so the row's `flex-wrap` puts the action on its own line instead. */}
                <div className="flex min-w-0 flex-1 flex-col gap-3 lg:min-w-[min(14rem,100%)]">
                  <p className="eyebrow eyebrow-primary flex items-center gap-2">
                    <Icon name="layers" size={20} className="shrink-0" />
                    {t('today.due.eyebrow')}
                  </p>
                  {/* H2, which is the frame's size for this line: the count is the
                      largest text in the block, one step above the h3 card headings
                      around it, because it is the one figure the page exists to
                      answer. */}
                  <h2>{t('today.due.count', { count: buckets.dueCards })}</h2>
                  {data.reviewDeck.course !== null || data.reviewDeck.title !== null ? (
                    /* `wrap-anywhere`: a course name is the student's own string, and
                       at 200% text "Kriptografi" is wider than the column it sits in.
                       `break-words` does not lower a box's min-content size, so it
                       leaves the word overflowing. */
                    <p className="wrap-anywhere text-label text-muted">
                      {[data.reviewDeck.course, data.reviewDeck.title]
                        .filter((part) => part !== null)
                        .join(' · ')}
                    </p>
                  ) : null}
                </div>
                <div className="flex flex-col gap-2">
                  <Link
                    to={`/study/${data.reviewDeckId}`}
                    className="btn btn-primary w-full lg:max-w-56"
                  >
                    <Icon name="cards" />
                    {t('today.dueCards.review', { count: buckets.dueCards })}
                  </Link>
                  {/* The queue this button opens, named, in the frame's own place
                      for it: the mono line under the action. It is not an estimate
                      of how long a review takes — nothing in the app knows that, and
                      a number here would be the one figure on this screen the API
                      never returned. */}
                  <p className="eyebrow lg:text-center">{t('today.dueCards.title')}</p>
                </div>
              </>
            ) : (
              <EmptyState
                title={t('today.dueCards.emptyTitle')}
                reason={t('today.dueCards.emptyReason')}
                padded={false}
              />
            )}
          </div>
        </div>
        {/* --- Quick capture --------------------------------------------------- */}
        <div className="order-2 flex flex-col gap-2 [&>*:first-child]:flex-1 lg:col-start-2 lg:row-start-3">
          <QuickCapture />
          {offline ? <p className="meta text-warning">{t('today.capture.offline')}</p> : null}
        </div>


        {/* --- Today's timeline ----------------------------------------------- */}
        <div className="order-3 flex flex-col [&>*:first-child]:flex-1 lg:col-start-1 lg:row-start-2">
          <div className="card flex flex-col gap-4 p-4 md:p-6">
            <WidgetHeading title={t('today.timeline.title')} />
            {/* The frame's summary row: what the day holds on the left, the zone it
                is read in on the right, both in the uppercase mono form the design
                types its metadata in. **The zone is not a second string inside the
                heading's row**: that arrangement measured 250px inside a 212px column
                at a 32px root and was clipped, because a card heading's `<h3>` cannot
                shrink out of a sibling's way. This row wraps instead of shrinking, so
                the two parts take separate lines before either is cut. */}
            <div className="flex flex-wrap items-baseline justify-between gap-x-4 gap-y-1">
              <p className="eyebrow break-words">
                {[
                  ...(rows.length === 0 ? [] : timelineCounts(rows, t)),
                  t('today.timeline.summaryMinutes', { count: buckets.minutesToday }),
                ].join(' · ')}
              </p>
              <p className="eyebrow">{zoneOffsetLabel(now, timeZone)}</p>
            </div>
            {rows.length === 0 ? (
              <EmptyState
                title={t('today.tasks.emptyTitle')}
                reason={t('today.tasks.emptyReason')}
                padded={false}
              />
            ) : (
              <ol className="flex flex-col gap-4">{timelineRows(rows, now, timeZone, t)}</ol>
            )}
          </div>
        </div>

        {/* --- The week's deadlines ------------------------------------------- */}
        <div className="order-4 flex flex-col [&>*:first-child]:flex-1 lg:col-start-2 lg:row-start-2">
          <div className="card flex flex-col gap-4 p-4 md:p-6">
            <WidgetHeading
              title={t('today.exams.title')}
              meta={buckets.thisWeek.length > 0 ? String(buckets.thisWeek.length) : undefined}
            />
            <p className="eyebrow">
              {[
                t('today.deadlines.weekRange', {
                  from: weekEdge(buckets.week.from, 'day'),
                  to: weekEdge(buckets.week.to, 'dayMonth'),
                }),
                ...(buckets.thisWeek.length === 0
                  ? []
                  : [t('today.deadlines.count', { count: buckets.thisWeek.length })]),
              ].join(' · ')}
            </p>
            {buckets.thisWeek.length === 0 ? (
              <EmptyState
                title={t('today.exams.emptyTitle')}
                reason={t('today.exams.emptyReason')}
                padded={false}
              />
            ) : (
              <ul className="flex flex-col gap-4">
                {buckets.thisWeek.map((event) => {
                  const course = courses.get(event.workspace_id ?? '')
                  const offset = dayOffset(
                    buckets.week.from,
                    localDateKey(new Date(event.starts_at), timeZone),
                    6,
                  )
                  return (
                    <li
                      key={event.id}
                      className="flex flex-col gap-3 rounded-field border border-border p-4"
                    >
                      <div className="flex flex-wrap items-center justify-between gap-2">
                        <CourseTag course={course} />
                        {offset !== null ? (
                          /* The frame's own tone: "Today" is amber and a later day is
                             muted. The word is the signal and the tone repeats it, so
                             nothing is carried by colour alone — and "In 2 days" is
                             not a warning, which is why only the zero offset takes it.
                             **`meta`, not `eyebrow`**: the frame renders this line in
                             sentence case ("Today", "In 2 days") where the week range
                             above it and the due line below are caps, and `.eyebrow`
                             is the uppercase form — plus it is defined as the line
                             *above* a heading, which this is not. `SPEC.md:82` asks for
                             sentence case in plain words. */
                          <span className={offset === 0 ? 'meta text-warning' : 'meta'}>
                            {offset === 0
                              ? t('today.deadlines.today')
                              : t('today.deadlines.inDays', { count: offset })}
                          </span>
                        ) : null}
                      </div>
                      <div className="flex flex-col gap-1">
                        <p className="text-label break-words">{event.title}</p>
                        {course !== undefined ? (
                          <p className="text-caption text-muted">{course.name}</p>
                        ) : null}
                      </div>
                      {/* The day and the time, in the reader's zone and in the
                          frame's own two-part form (`MON 12 OCT · 18:00`).
                          `starts_at` is a UTC instant, so printing it raw names the
                          wrong day for a deadline before 07:00 in Jakarta. */}
                      <p className="eyebrow">
                        {[
                          formatInZone(new Date(event.starts_at), timeZone, {
                            weekday: 'short',
                            day: 'numeric',
                            month: 'short',
                          }),
                          formatInZone(new Date(event.starts_at), timeZone, {
                            hour: '2-digit',
                            minute: '2-digit',
                            hour12: false,
                          }),
                        ].join(' · ')}
                      </p>
                    </li>
                  )
                })}
              </ul>
            )}
            <Link to="/planner" className="btn btn-secondary w-full">
              <Icon name="calendar" />
              {t('today.deadlines.openPlanner')}
            </Link>
          </div>
        </div>

        {/* --- Focus timer ----------------------------------------------------- */}
        <div className="order-5 flex flex-col [&>*:first-child]:flex-1 lg:col-start-1 lg:row-start-3">
          <FocusTimer />
        </div>
        {/* --- Rhythm --------------------------------------------------------- */}
        <div className="order-6 flex flex-col [&>*:first-child]:flex-1 lg:col-start-2 lg:row-start-1">
          <div className="card flex flex-col gap-4 p-6">
            {data.loading ? (
              <LoadingState label={t('today.activity.loading')} />
            ) : (
              <>
                <h3 className="flex items-center gap-3">
                  <Icon name="streak" size={24} className="shrink-0 text-warning" />
                  {buckets.streakDays > 0
                    ? t('today.activity.streak', { count: buckets.streakDays })
                    : t('today.streak.emptyTitle')}
                </h3>
                <ol className="flex flex-wrap justify-between gap-2">
                  {buckets.recentDays.map((day) => (
                    <RecentDayCell key={day.key} day={day} />
                  ))}
                </ol>
                <p className="text-small text-subtle">
                  {buckets.streakDays > 0
                    ? t('today.streak.encouragement')
                    : t('today.streak.emptyReason')}
                </p>
              </>
            )}
          </div>
        </div>

      </div>

      {buckets.overdue.length > 0 ? (
        <Section title={t('today.overdue.title')} count={buckets.overdue.length}>
          <ul className="card flex flex-col gap-3 p-4">
            {buckets.overdue.map((task) => (
              <li key={task.id} className="flex flex-wrap items-baseline justify-between gap-2">
                <span className="text-body break-words">{task.title}</span>
                {task.due_at !== null ? (
                  <span className="meta">
                    {t('today.overdue.due', {
                      when: formatInZone(new Date(task.due_at), timeZone, { dateStyle: 'medium' }),
                    })}
                  </span>
                ) : null}
              </li>
            ))}
          </ul>
        </Section>
      ) : null}

      <PageFooter
        left={`${timeZone.toUpperCase()} · ${zoneOffsetLabel(now, timeZone)}`}
        right={savedState(report, timeZone, t)}
      />
    </div>
  )
}

/**
 * One timeline row, as the screen renders it.
 *
 * Built once from the entry and the course it belongs to, so the row component
 * holds no lookup logic: the title, the kind word and the detail line are all
 * facts of the row, and a component that resolved them itself would be the second
 * place that decides what a task row and an event row each show.
 */
interface TimelineRow {
  id: string
  at: string
  /** The end instant for an event, null for a task — a task has a due moment. */
  endAt: string | null
  /**
   * What the row is, as the summary counts it: the event's `kind` for an event, and
   * `task` for a task. The frame's summary line counts rows by kind ("2 classes · 2
   * study blocks"), so the count and the rows cannot disagree about what the day is
   * made of.
   */
  kind: string
  title: string
  kindWord: string
  course: CourseRef | undefined
  detail: string | null
  /**
   * The row's fill, which is the frame's own distinction: a deadline is not a class
   * or a study block, and the design draws it on the warning tint rather than on the
   * raised indigo one. It is a tone *and* the word "Deadline" beside it, never the
   * tone alone.
   */
  tone: 'raised' | 'warning'
}

function timelineRow(
  entry: TodayEntry,
  course: CourseRef | undefined,
  t: TFunction,
  now: Date,
): TimelineRow {
  if (entry.kind === 'task') {
    return {
      id: entry.id,
      at: entry.at,
      endAt: null,
      kind: 'task',
      title: entry.task.title,
      kindWord: t('today.timeline.kindTask'),
      course,
      detail:
        entry.task.estimate_min === null
          ? null
          : t('today.capture.estimate', { count: entry.task.estimate_min }),
      tone: 'raised',
    }
  }
  return {
    id: entry.id,
    at: entry.at,
    endAt: entry.event.ends_at,
    kind: entry.event.kind,
    title: entry.event.title,
    kindWord: [
      t(`calendar.kind.${entry.event.kind}`),
      // Whether a class is over is a fact of the day, not a status the student
      // set: the frame marks the finished one, and `ends_at` is where that comes
      // from rather than from a guess about the hour. Read against the screen's
      // own `now`, so the mark cannot change between two renders of one view.
      Date.parse(entry.event.ends_at) <= now.getTime() ? t('today.timeline.finished') : null,
    ]
      .filter((part) => part !== null)
      .join(' · '),
    course,
    detail: course?.name ?? null,
    tone: entry.event.kind === 'deadline' ? 'warning' : 'raised',
  }
}

/** One day of the streak strip: the weekday above a 28px mark. */
function RecentDayCell({ day }: { day: RecentDay }) {
  const { t } = useTranslation()
  // The key is a calendar date, so it is formatted in UTC: reading it in the
  // student's zone would shift it a day for anyone west of Greenwich, and the
  // strip would then label yesterday's cell "Thu" on a Thursday.
  const weekday = formatInZone(new Date(`${day.key}T00:00:00Z`), 'UTC', { weekday: 'short' })

  return (
    <li className="flex flex-col items-center gap-2">
      <span className="meta">{weekday}</span>
      {/* The mark is decorative and the state is in the text below it, so the
          reading survives monochrome and reaches a screen reader. */}
      <span
        className={
          day.active
            ? 'flex size-7 items-center justify-center rounded-full bg-primary-tint text-primary-text'
            : 'flex size-7 items-center justify-center rounded-full border border-border bg-surface'
        }
        aria-hidden="true"
      >
        {day.active ? (
          <Icon name="check" size={16} />
        ) : (
          <span className="size-1 rounded-full bg-border-strong" />
        )}
      </span>
      <span className="sr-only">
        {day.active ? t('today.streak.dayActive') : t('today.streak.dayIdle')}
      </span>
    </li>
  )
}

/** The course tag: the student's own code, or the course name when there is none. */
function CourseTag({ course }: { course: CourseRef | undefined }) {
  if (course === undefined) return null
  // The hue is the server's index into the six course colours, and the pair is a
  // reference the stylesheet resolves, so the dark theme's swap applies without
  // this component knowing there is one.
  const pair = COURSE_VARS[COURSE_NAMES[course.color] ?? 'indigo']
  return (
    <span
      className="chip font-mono text-meta"
      style={{ backgroundColor: pair.tint, color: pair.marker }}
    >
      {course.code ?? course.name}
    </span>
  )
}

/** `12` or `12 October`, read in UTC because the input is a calendar date. */
function weekEdge(key: string, style: 'day' | 'dayMonth'): string {
  return formatInZone(
    new Date(`${key}T00:00:00Z`),
    'UTC',
    style === 'day' ? { day: 'numeric' } : { day: 'numeric', month: 'long' },
  )
}

/**
 * The timeline's mono summary: what today holds, counted from the rows the timeline
 * renders.
 *
 * Counted here rather than from the events alone, so the line and the list below it
 * cannot disagree about what is on today. **Counted by kind**, which is the frame's
 * own arrangement ("2 classes · 2 study blocks"): a day of two lectures and one
 * deadline reads as that, and the counts come from the same `kind` each row's own
 * word is drawn from. The four kinds with a noun of their own are counted; a row
 * from a connected calendar is counted under one label rather than pluralized,
 * because `calendar.kind.external` is a sentence and not a noun.
 */
const TIMELINE_KINDS = ['class', 'exam', 'study_block', 'deadline'] as const

function timelineCounts(rows: TimelineRow[], t: TFunction): string[] {
  const parts: string[] = []
  for (const kind of TIMELINE_KINDS) {
    const count = rows.filter((row) => row.kind === kind).length
    if (count > 0) parts.push(t(`today.timeline.kindCount.${kind}`, { count }))
  }
  const tasks = rows.filter((row) => row.kind === 'task').length
  if (tasks > 0) parts.push(t('today.timeline.summaryTasks', { count: tasks }))
  const external = rows.filter(
    (row) => row.kind !== 'task' && !TIMELINE_KINDS.some((kind) => kind === row.kind),
  ).length
  if (external > 0) parts.push(t('today.timeline.kindCount.external', { count: external }))
  return parts
}

/**
 * The timeline's rows, with the current time placed among them.
 *
 * The "now" rule goes before the first entry that has not started yet, which is
 * what makes it a position on the day rather than a clock pinned to the top. It
 * is drawn from the same `now` the buckets were computed from, so it cannot
 * disagree with the list about which day this is.
 */
function timelineRows(rows: TimelineRow[], now: Date, timeZone: string, t: TFunction): ReactNode[] {
  const out: ReactNode[] = []
  let placed = false
  for (const row of rows) {
    if (!placed && Date.parse(row.at) > now.getTime()) {
      out.push(<NowRule key="now" now={now} timeZone={timeZone} />)
      placed = true
    }
    out.push(
      <li key={row.id} className="flex flex-wrap gap-3 md:gap-4">
        <div className="flex w-13 shrink-0 flex-col">
          <span className="meta">{clock(row.at, timeZone)}</span>
          {row.endAt !== null ? <span className="meta">{clock(row.endAt, timeZone)}</span> : null}
        </div>
        {/* `min-w-[min(10rem,100%)]`, not `min-w-0`, and the two together with the row's
            own `flex-wrap` are what the 200%-text case needs. The row is `[time | body]`,
            and the time column is `w-13` — **3.25rem, so 104px at a 32px root** — beside a
            `gap-4` (32px) and the body's `p-3` (48px): 184px of a phone card's 214px content
            box is chrome, leaving 30px for the body. With `min-w-0` the body simply shrank
            into that and the category row inside it was clipped — measured in Chromium at
            390px/200%, `scrollWidth` **53** against a `clientWidth` of **28**, both themes,
            and reachable only once a fixture put a row on today's timeline. The capped rem
            floor is the same instrument `QuizPanel` and `DevicesScreen` use: at 100% it is
            160px, which is under the room available so nothing changes, and at 200% it
            exceeds the leftover width, so the row **wraps the body onto its own line** and
            the body gets the full 214px. `min(…, 100%)` is what keeps the floor from
            exceeding its own container below `lg`. */}
        <div
          className={`${
            row.tone === 'warning' ? 'bg-warning-tint' : 'raised'
          } flex min-w-[min(10rem,100%)] flex-1 flex-col gap-2 rounded-field p-3`}
        >
          <div className="flex flex-wrap items-center justify-between gap-2">
            <CourseTag course={row.course} />
            {/* The kind word sits at the row's right end, which is the frame's
                arrangement: the tag names what the row belongs to, the word names
                what it is. `ml-auto` rather than relying on `justify-between`,
                because a row with no course renders no tag and a lone child of a
                `justify-between` row stays at the left. */}
            <span className="eyebrow ml-auto wrap-anywhere">{row.kindWord}</span>
          </div>
          <p className="text-label break-words">
            <span className="sr-only">
              {t('today.timeline.at', {
                when: formatInZone(new Date(row.at), timeZone, {
                  dateStyle: 'medium',
                  timeStyle: 'short',
                }),
              })}
              {': '}
            </span>
            {row.title}
          </p>
          {row.detail !== null ? (
            <p className="text-caption text-muted break-words">{row.detail}</p>
          ) : null}
        </div>
      </li>,
    )
  }
  if (!placed) out.push(<NowRule key="now" now={now} timeZone={timeZone} />)
  return out
}

/** `09:00`, in the reader's zone. */
function clock(instant: string, timeZone: string): string {
  return formatInZone(new Date(instant), timeZone, {
    hour: '2-digit',
    minute: '2-digit',
    hour12: false,
  })
}

/** The indigo rule the current time falls on. */
function NowRule({ now, timeZone }: { now: Date; timeZone: string }) {
  const { t } = useTranslation()
  return (
    <li className="flex items-center gap-3">
      <span className="meta w-13 shrink-0 text-primary-text">
        {clock(now.toISOString(), timeZone)}
      </span>
      <span className="h-px flex-1 bg-primary" />
      <span className="eyebrow text-primary-text">{t('today.timeline.now')}</span>
    </li>
  )
}

/**
 * The footer's saved state.
 *
 * The word comes from the shell's own report and never from a guess, and the
 * offline line is the frame's: what is on this device, and that it is waiting.
 */
function savedState(
  report: { state: string; lastSyncedAt: string | null } | null,
  timeZone: string,
  t: TFunction,
): string | undefined {
  if (report === null) return undefined
  if (report.state === 'offline') return t('today.footer.savedOnDevice')
  if (report.lastSyncedAt !== null) {
    return t('sync.lastSynced', {
      when: formatInZone(new Date(report.lastSyncedAt), timeZone, {
        dateStyle: 'medium',
        timeStyle: 'short',
      }),
    })
  }
  return t(`sync.state.${report.state}`)
}

// ---------------------------------------------------------------------------
// DatePickerPopover – icon-only button that opens a month calendar grid
// ---------------------------------------------------------------------------

/** Build the 6×7 grid for a given month, padding with prev/next month days. */
function monthGrid(year: number, month: number): Date[] {
  const first = new Date(Date.UTC(year, month, 1))
  // Monday = 0 … Sunday = 6
  const startDay = (first.getUTCDay() + 6) % 7
  const cells: Date[] = []
  for (let i = -startDay; cells.length < 42; i++) {
    cells.push(new Date(Date.UTC(year, month, 1 + i, 12)))
  }
  return cells
}

const WEEKDAYS_SHORT = ['Mon', 'Tue', 'Wed', 'Thu', 'Fri', 'Sat', 'Sun'] as const

function DatePickerPopover({
  now,
  selectedKey,
  realTodayKey,
  onSelect,
}: {
  now: Date
  selectedKey: string
  realTodayKey: string
  onSelect: (date: Date | null) => void
}) {
  const { t } = useTranslation()
  const [open, setOpen] = useState(false)

  // The month being viewed in the popover (may differ from now when navigating).
  const [viewYear, setViewYear] = useState(now.getUTCFullYear())
  const [viewMonth, setViewMonth] = useState(now.getUTCMonth())

  // Reset the viewed month to today's when the popover opens.
  const toggle = useCallback(() => {
    setOpen((prev) => {
      if (!prev) {
        // Parse the selected key to open on the currently viewed month.
        const [y, m] = selectedKey.split('-').map(Number)
        setViewYear(y!)
        setViewMonth((m ?? 1) - 1)
      }
      return !prev
    })
  }, [selectedKey])

  const popoverRef = useRef<HTMLDivElement>(null)
  const buttonRef = useRef<HTMLButtonElement>(null)

  // Close on outside click or Escape.
  useEffect(() => {
    if (!open) return
    function onPointerDown(e: PointerEvent) {
      if (
        popoverRef.current && !popoverRef.current.contains(e.target as Node)
        && buttonRef.current && !buttonRef.current.contains(e.target as Node)
      ) {
        setOpen(false)
      }
    }
    function onKeyDown(e: KeyboardEvent) {
      if (e.key === 'Escape') { setOpen(false); buttonRef.current?.focus() }
    }
    document.addEventListener('pointerdown', onPointerDown, true)
    document.addEventListener('keydown', onKeyDown)
    return () => {
      document.removeEventListener('pointerdown', onPointerDown, true)
      document.removeEventListener('keydown', onKeyDown)
    }
  }, [open])

  const cells = useMemo(() => monthGrid(viewYear, viewMonth), [viewYear, viewMonth])

  const monthLabel = new Intl.DateTimeFormat('default', { month: 'long', year: 'numeric', timeZone: 'UTC' })
    .format(new Date(Date.UTC(viewYear, viewMonth, 15)))

  const prevMonth = () => {
    setViewMonth((m) => { if (m === 0) { setViewYear((y) => y - 1); return 11 } return m - 1 })
  }
  const nextMonth = () => {
    setViewMonth((m) => { if (m === 11) { setViewYear((y) => y + 1); return 0 } return m + 1 })
  }

  return (
    <div className="date-picker-anchor">
      <button
        ref={buttonRef}
        type="button"
        className="btn btn-secondary shrink-0"
        aria-label={t('today.datePicker.openLabel')}
        aria-expanded={open}
        onClick={toggle}
      >
        <Icon name="calendar" />
      </button>
      {open && (
        <div ref={popoverRef} className="date-picker-popover" role="dialog" aria-label={t('today.datePicker.label')}>
          <div className="date-picker-header">
            <button
              type="button"
              className="date-picker-nav"
              aria-label={t('today.datePicker.prevMonth')}
              onClick={prevMonth}
            >
              <Icon name="chevron-left" size={16} />
            </button>
            <span className="date-picker-month-label">{monthLabel}</span>
            <button
              type="button"
              className="date-picker-nav"
              aria-label={t('today.datePicker.nextMonth')}
              onClick={nextMonth}
            >
              <Icon name="chevron-right" size={16} />
            </button>
          </div>
          <div className="date-picker-grid" role="grid" aria-label={monthLabel}>
            {WEEKDAYS_SHORT.map((wd) => (
              <span key={wd} className="date-picker-weekday" role="columnheader">{wd}</span>
            ))}
            {cells.map((date) => {
              const key = localDateKey(date, 'UTC')
              const inMonth = date.getUTCMonth() === viewMonth
              const isSelected = key === selectedKey
              const isRealToday = key === realTodayKey
              return (
                <button
                  key={key}
                  type="button"
                  className={
                    'date-picker-cell'
                    + (isSelected ? ' date-picker-cell-selected' : '')
                    + (isRealToday && !isSelected ? ' date-picker-cell-today' : '')
                    + (!inMonth ? ' date-picker-cell-outside' : '')
                  }
                  aria-current={isSelected ? 'date' : undefined}
                  aria-label={formatInZone(date, 'UTC', { weekday: 'long', day: 'numeric', month: 'long', year: 'numeric' })}
                  onClick={() => {
                    // Tapping "today" resets to null (same as date-strip).
                    onSelect(isRealToday ? null : date)
                    setOpen(false)
                  }}
                >
                  {date.getUTCDate()}
                </button>
              )
            })}
          </div>
        </div>
      )}
    </div>
  )
}
