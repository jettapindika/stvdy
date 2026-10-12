import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useDashboard, useTopicMastery } from '@/api/queries/dashboard'
import { useUsage } from '@/api/queries/usage'
import { asApiError } from '@/api/errors'
import { PageHeader, Section } from '@/components/layout/Section'
import { LockedFeature } from '@/components/ui/LockedFeature'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { localDateKey } from '@/lib/time'

/**
 * The dashboard: what the student actually did, counted from their own rows.
 *
 * **Every figure here traces to a row** (A289, roadmap gate 7(b)). There is no
 * streak invented on this screen, no composite score, no predicted grade, no
 * "retention", and no percentage the app cannot compute — the two routes return
 * counts and sums over rows the student owns, and this screen renders them and
 * adds nothing. A topic with no quiz attempts is **absent** from the breakdown
 * rather than shown as 0%, which is the same rule in its most visible form: an
 * uncomputable number must not reach the screen.
 *
 * **Every chart has a text equivalent.** Each figure carries a sentence beside it
 * that states the same fact in words — "6 courses studied, 210 minutes in total"
 * — so the screen is readable without the bars, and the bars are decoration over
 * a number rather than the only place the number exists. That is what makes the
 * screen usable with a screen reader and at 200% text scale at once.
 *
 * **The topic breakdown is a separate route because it is a separate plan
 * feature.** `GET /dashboard/topics` needs `Feature::AdvancedAnalytics` (Power)
 * and answers `403 plan_required` below it, so the locked card is rendered from
 * `GET /usage`'s `features.advanced_analytics` and **no number from that route is
 * on screen** on a plan that does not grant it. One route with a flag would have
 * made the whole dashboard a 403 on Free.
 *
 * **The window is the student's local days** (A6): the caller computes it in the
 * account's zone, because a session at 23:30 Jakarta is today and a UTC window
 * would ask the server for yesterday.
 */

/** How many days back the window reaches. Two weeks is what a "progress" screen means. */
const WINDOW_DAYS = 14

/** A horizontal bar, whose width is the figure's share of the largest one. */
function Bar({ share, label }: { share: number; label: string }) {
  return (
    <div
      className="h-2 min-w-0 flex-1 rounded-field bg-surface-raised"
      role="img"
      aria-label={label}
    >
      <div
        className="h-2 rounded-field bg-primary"
        // A width as a percentage of the largest row. It is inline rather than a
        // utility because the value is data, not a design token, and no colour is
        // involved: the bar's own label is beside it.
        style={{ width: `${String(Math.round(share * 100))}%` }}
      />
    </div>
  )
}

export function DashboardScreen() {
  const { t } = useTranslation()
  const session = useSession()
  const usage = useUsage()
  const [now] = useState(() => new Date())

  const timeZone =
    session.status === 'signed-in' && session.user.timezone.length > 0
      ? session.user.timezone
      : 'Asia/Jakarta'

  // The window is the student's local days, which is why the keys are built here
  // rather than on the server: `YYYY-MM-DD` in the account's zone.
  const to = localDateKey(now, timeZone)
  const from = localDateKey(new Date(now.getTime() - WINDOW_DAYS * 24 * 60 * 60 * 1000), timeZone)

  const advanced = usage.data?.features.advanced_analytics === true
  const summary = useDashboard(from, to)
  const topics = useTopicMastery(advanced)

  const summaryError = asApiError(summary.error)
  const topicsError = asApiError(topics.error)

  const totals = summary.data?.totals
  const courses = summary.data?.time_per_course ?? []
  const reviews = summary.data?.review_activity ?? []
  const busiestCourse = Math.max(1, ...courses.map((course) => course.minutes))
  const busiestDay = Math.max(1, ...reviews.map((day) => day.reviews))

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <div className="flex flex-col gap-1">
        <PageHeader title={t('dashboard.title')} subtitle={t('dashboard.subtitle')} />
        {/* The window, in words: the figures below are counts over these local days,
            and a reader has to be able to see which days they cover. */}
        <p className="meta">{t('dashboard.window', { from, to })}</p>
      </div>

      {summary.isPending && <LoadingState label={t('dashboard.loading')} />}

      {summaryError !== null && (
        <ApiErrorNotice
          error={summaryError}
          noun={t('dashboard.noun')}
          action={
            <button
              type="button"
              className="btn btn-secondary min-h-11"
              onClick={() => void summary.refetch()}
            >
              {t('actions.retry')}
            </button>
          }
        />
      )}

      {totals !== undefined && (
        <Section title={t('dashboard.totals')}>
          <div className="card flex flex-col gap-3 p-4">
            <dl className="grid grid-cols-2 gap-3">
              {[
                { key: 'minutes', value: totals.minutes_studied },
                { key: 'reviews', value: totals.reviews },
                { key: 'cardsDue', value: totals.cards_due },
                { key: 'quizzes', value: totals.quizzes_taken },
              ].map((row) => (
                <div key={row.key} className="flex flex-col gap-0.5">
                  <dt className="label">{t(`dashboard.total.${row.key}`)}</dt>
                  <dd className="text-base">{row.value}</dd>
                </div>
              ))}
            </dl>
            {/* The sentence is the chart's equivalent and the section's summary: a
                reader who never looks at the numbers below still learns the one
                fact they carry. */}
            <p className="text-small text-muted">
              {t('dashboard.summary', {
                minutes: totals.minutes_studied,
                reviews: totals.reviews,
              })}
            </p>
          </div>
        </Section>
      )}

      {summary.data !== undefined && (
        <Section title={t('dashboard.timePerCourse')}>
          <div className="card flex flex-col gap-3 p-4">
            {courses.length === 0 ? (
              <p className="text-small text-muted">{t('dashboard.noCourses')}</p>
            ) : (
              <>
                <p className="text-small text-muted">
                  {t('dashboard.timePerCourseSummary', {
                    courses: courses.length,
                    minutes: totals?.minutes_studied ?? 0,
                  })}
                </p>
                <ul className="flex flex-col gap-2">
                  {courses.map((course) => (
                    <li key={course.workspace_id} className="flex items-center gap-3">
                      <span className="min-w-0 flex-1 truncate text-small">{course.title}</span>
                      <Bar
                        share={course.minutes / busiestCourse}
                        label={t('dashboard.shareLabel', {
                          title: course.title,
                          percent: Math.round((course.minutes / busiestCourse) * 100),
                        })}
                      />
                      <span className="meta shrink-0">
                        {t('dashboard.minutes', { count: course.minutes })}
                      </span>
                    </li>
                  ))}
                </ul>
              </>
            )}
          </div>
        </Section>
      )}

      {summary.data !== undefined && (
        <Section title={t('dashboard.reviewActivity')}>
          <div className="card flex flex-col gap-3 p-4">
            {reviews.length === 0 ? (
              <p className="text-small text-muted">{t('dashboard.noReviews')}</p>
            ) : (
              <>
                <p className="text-small text-muted">
                  {t('dashboard.reviewActivitySummary', {
                    days: reviews.length,
                    reviews: reviews.reduce((sum, day) => sum + day.reviews, 0),
                  })}
                </p>
                <ul className="flex flex-col gap-2">
                  {reviews.map((day) => (
                    <li key={day.local_date} className="flex items-center gap-3">
                      <span className="min-w-0 flex-1 text-small">{day.local_date}</span>
                      <Bar
                        share={day.reviews / busiestDay}
                        label={t('dashboard.reviews', { count: day.reviews })}
                      />
                      <span className="meta shrink-0">
                        {t('dashboard.reviews', { count: day.reviews })}
                      </span>
                    </li>
                  ))}
                </ul>
              </>
            )}
          </div>
        </Section>
      )}

      {summary.data !== undefined &&
        totals !== undefined &&
        totals.minutes_studied === 0 &&
        totals.reviews === 0 &&
        totals.quizzes_taken === 0 && <EmptyState title={t('dashboard.title')} reason={t('dashboard.empty')} />}

      <Section title={t('dashboard.mastery')}>
        {!advanced ? (
          // The plan's answer, from `GET /usage` — never the route's 403, which
          // would be a failed request on a screen whose whole point is that it
          // works on every plan. No number from `/dashboard/topics` is on screen
          // here, because the query is not even enabled.
          <LockedFeature
            whatKey="dashboard.mastery"
            minPlan="Power"
            reasonKey="dashboard.lockedReason"
          />
        ) : topics.isPending ? (
          <LoadingState label={t('dashboard.loading')} />
        ) : topicsError !== null ? (
          <ApiErrorNotice
            error={topicsError}
            noun={t('dashboard.noun')}
            action={
              <button
                type="button"
                className="btn btn-secondary min-h-11"
                onClick={() => void topics.refetch()}
              >
                {t('actions.retry')}
              </button>
            }
          />
        ) : (
          <div className="flex flex-col gap-4">
            <div className="card flex flex-col gap-3 p-4">
              {(topics.data?.quiz_mastery ?? []).length === 0 ? (
                <p className="text-small text-muted">{t('dashboard.noMastery')}</p>
              ) : (
                <>
                  <p className="text-small text-muted">
                    {t('dashboard.masterySummary', {
                      topics: (topics.data?.quiz_mastery ?? []).length,
                    })}
                  </p>
                  <ul className="flex flex-col gap-2">
                    {(topics.data?.quiz_mastery ?? []).map((topic) => (
                      <li key={topic.source_id} className="flex flex-col gap-1">
                        <span className="text-small break-words">{topic.title}</span>
                        <span className="meta">
                          {t('dashboard.masteryLabel', {
                            title: topic.title,
                            correct: topic.correct,
                            attempts: topic.attempts,
                            accuracy: Math.round(topic.accuracy * 100),
                          })}
                        </span>
                      </li>
                    ))}
                  </ul>
                </>
              )}
            </div>

            {(topics.data?.weak_topics ?? []).length > 0 && (
              <div className="card flex flex-col gap-3 p-4">
                <h3>{t('dashboard.weakTopics')}</h3>
                <p className="text-small text-muted">{t('dashboard.weakTopicsHint')}</p>
                <ul className="flex flex-col gap-2">
                  {(topics.data?.weak_topics ?? []).map((topic) => (
                    <li key={topic.source_id} className="flex flex-wrap items-center gap-2">
                      <span className="min-w-0 flex-1 break-words text-small">{topic.title}</span>
                      <span className="meta">
                        {t('dashboard.masteryLabel', {
                          title: topic.title,
                          correct: topic.correct,
                          attempts: topic.attempts,
                          accuracy: Math.round(topic.accuracy * 100),
                        })}
                      </span>
                    </li>
                  ))}
                </ul>
              </div>
            )}
          </div>
        )}
      </Section>
    </div>
  )
}
