import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { useSyncStatus } from '@/api/queries/sync'
import { PageHeader, Section } from '@/components/layout/Section'
import { EmptyState, LoadingState } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { FocusTimer } from './FocusTimer'
import { QuickCapture } from './QuickCapture'
import { todayBuckets } from './day'
import { useTodayData } from './useTodayData'

/**
 * Today, the default home.
 *
 * Four things this screen is built to get right, and each is a gate rather than a
 * preference.
 *
 * **Every number traces to a row** (gate 7). "N due" is the number of cards in the
 * review queue, "N min today" is the sum of today's `study_sessions.duration_min`,
 * the streak is consecutive local days that hold a review, an attempt or a
 * session, and the checklist's counts are live workspaces, sources and decks. Not
 * one is a placeholder, and the buckets are computed by `day.ts`, which is a pure
 * function of the rows the device already holds — which is also why this screen
 * works with the network down (A205).
 *
 * **Nothing is invented while loading.** A pending read renders a loading state,
 * not zeroes: a screen that showed "0 due" while the read was in flight would be
 * telling the student they have nothing to do.
 *
 * **The sync line says only what the shell knows.** It reads the status the shell
 * derives from stored facts and, when that status is `offline`, the reason the
 * locale already carries. When there is no report at all it says nothing, because
 * "we do not know" is not "you are offline".
 *
 * **The sections this phase does not own are named, not faked.** Study blocks are
 * the planner's (Phase 11), and the section says so rather than showing a
 * permanently empty list that looks like a feature that found nothing.
 */
export function TodayScreen() {
  const { t } = useTranslation()
  const session = useSession()
  const timeZone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE
  // The instant this view of the day was opened, fixed for the life of the mount.
  // It is the same instant the day buckets are computed from and the one the read
  // window is derived from, so the two cannot disagree — and a window that changed
  // on every render would be a new query on every render, which is a screen that
  // never settles. Navigating away and back recomputes it.
  const [now] = useState(() => new Date())

  // A superset of the local day and the next seven, so the *server* sends
  // everything the local bucketing below might need. The window is deliberately
  // not computed from the local day's exact instants: that needs the zone's
  // offset arithmetic here, and a bug in it would silently ask for a window the
  // rows are not in — the failure mode `day.ts` cannot see. What the window does
  // not decide is which day a row is on: `todayBuckets` decides that, in the
  // student's zone.
  const window = {
    from: new Date(now.getTime() - 24 * 60 * 60 * 1000).toISOString(),
    to: new Date(now.getTime() + 8 * 24 * 60 * 60 * 1000).toISOString(),
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

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader title={t('nav.today')} subtitle={t('today.subtitle')} />

      {/* The sync line: the state as a word, and the reason when there is one. */}
      {report !== null ? (
        <p className="text-small text-muted">
          {report.state === 'offline'
            ? `${t('states.offline')} — ${t('states.offlineReason')}`
            : t(
                report.state === 'syncing'
                  ? 'states.syncing'
                  : report.state === 'synced'
                    ? 'states.synced'
                    : 'states.needsAttention',
              )}
        </p>
      ) : null}

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

      <Section title={t('today.dueCards.title')} count={buckets.dueCards > 0 ? buckets.dueCards : undefined}>
        <div className="card flex flex-col gap-3 p-4">
          {data.loading ? (
            <LoadingState label={t('today.dueCards.loading')} />
          ) : buckets.dueCards > 0 && data.reviewDeckId !== null ? (
            <Link
              to={`/study/${data.reviewDeckId}`}
              className="link inline-flex min-h-11 items-center self-start"
            >
              {t('today.dueCards.review', { count: buckets.dueCards })}
            </Link>
          ) : (
            <EmptyState
              title={t('today.dueCards.emptyTitle')}
              reason={t('today.dueCards.emptyReason')}
              padded={false}
            />
          )}
        </div>
      </Section>

      <Section
        title={t('today.timeline.title')}
        count={buckets.today.length > 0 ? buckets.today.length : undefined}
      >
        <div className="card p-4">
          {buckets.today.length === 0 ? (
            <EmptyState
              title={t('today.tasks.emptyTitle')}
              reason={t('today.tasks.emptyReason')}
              padded={false}
            />
          ) : (
            <ul className="flex flex-col gap-3">
              {buckets.today.map((entry) => (
                <li key={entry.id} className="flex flex-wrap items-baseline justify-between gap-2">
                  <span className="text-body">
                    {entry.kind === 'task' ? entry.task.title : entry.event.title}
                  </span>
                  <span className="meta">
                    {t('today.timeline.at', {
                      when: formatInZone(new Date(entry.at), timeZone, {
                        dateStyle: 'medium',
                        timeStyle: 'short',
                      }),
                    })}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </Section>

      {buckets.overdue.length > 0 ? (
        <Section title={t('today.overdue.title')} count={buckets.overdue.length}>
          <ul className="card flex flex-col gap-3 p-4">
            {buckets.overdue.map((task) => (
              <li key={task.id} className="flex flex-wrap items-baseline justify-between gap-2">
                <span className="text-body">{task.title}</span>
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

      <Section
        title={t('today.exams.title')}
        count={buckets.thisWeek.length > 0 ? buckets.thisWeek.length : undefined}
      >
        <div className="card p-4">
          {buckets.thisWeek.length === 0 ? (
            <EmptyState
              title={t('today.exams.emptyTitle')}
              reason={t('today.exams.emptyReason')}
              padded={false}
            />
          ) : (
            <ul className="flex flex-col gap-3">
              {buckets.thisWeek.map((event) => (
                <li key={event.id} className="flex flex-wrap items-baseline justify-between gap-2">
                  <span className="text-body">{event.title}</span>
                  <span className="meta">
                    {formatInZone(new Date(event.starts_at), timeZone, { dateStyle: 'medium' })}
                  </span>
                </li>
              ))}
            </ul>
          )}
        </div>
      </Section>

      {data.lastWorkspace !== null ? (
        <Section title={t('today.continue.title')}>
          <div className="card flex flex-col gap-2 p-4">
            <Link
              to={`/workspaces/${data.lastWorkspace.id}`}
              className="link inline-flex min-h-11 items-center self-start"
            >
              {t('today.continue.open', { name: data.lastWorkspace.name })}
            </Link>
            {/* The workspace's own `updated_at`, never a clock read here: "last
                touched just now" would be a fact this screen does not have. A
                cached row with no timestamp shows no line rather than an
                invented one. */}
            {data.lastWorkspace.updatedAt !== null ? (
              <p className="meta">
                {t('today.continue.lastTouched', {
                  when: formatInZone(new Date(data.lastWorkspace.updatedAt), timeZone, {
                    dateStyle: 'medium',
                  }),
                })}
              </p>
            ) : null}
          </div>
        </Section>
      ) : null}

      <Section title={t('today.activity.title')}>
        <div className="card flex flex-col gap-2 p-4">
          <p className="text-body">{t('today.activity.minutes', { count: buckets.minutesToday })}</p>
          <p className="text-body">{t('today.activity.streak', { count: buckets.streakDays })}</p>
        </div>
      </Section>

      <Section title={t('today.timer.title')}>
        <FocusTimer />
      </Section>

      <Section title={t('today.capture.title')}>
        <QuickCapture />
      </Section>

      <Section title={t('today.studyBlocks.title')}>
        <div className="card flex flex-col gap-2 p-4">
          <EmptyState
            title={t('today.studyBlocks.emptyTitle')}
            reason={t('today.studyBlocks.emptyReason')}
            padded={false}
          />
          <p className="text-small text-subtle">{t('today.studyBlocks.planner')}</p>
        </div>
      </Section>
    </div>
  )
}
