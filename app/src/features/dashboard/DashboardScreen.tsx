import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { useDashboard, useTopicMastery } from '@/api/queries/dashboard'
import { useUsage } from '@/api/queries/usage'
import { asApiError } from '@/api/errors'
import { PageHeader, Section, WidgetHeading } from '@/components/layout/Section'
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
 * screen usable with a screen reader and at 200% text scale at once. The bar's own
 * accessible name is the row's figure for the same reason: its *width* is that
 * row's share of the busiest row, and a name that called that share a percentage
 * "of your study time" would put a number on screen the route never returned.
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
 *
 * **The redesign's reading column, and why the two charts share a section.** One
 * `max-w-3xl` column, a `PageHeader` whose eyebrow carries the window, and one
 * `Section` per group of figures with a `card p-6` per figure. Time per course and
 * review activity are two widgets under one "study time" section because each
 * needs its own 20/26 heading (they are two figures over the same days, side by
 * side) and a section per chart would have put the chart's own name directly under
 * an identical section heading — the same words twice, which the design's
 * eyebrow rule forbids for exactly this reason. The totals card is headed by its
 * section, so it carries no card heading of its own.
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
  // Empty below the plan gate too: the query is disabled, so `topics.data` is
  // undefined and neither list has a row to render.
  const mastery = topics.data?.quiz_mastery ?? []
  const weakTopics = topics.data?.weak_topics ?? []
  const busiestCourse = Math.max(1, ...courses.map((course) => course.minutes))
  const busiestDay = Math.max(1, ...reviews.map((day) => day.reviews))

  // Nothing logged in the window. `cards_due` is deliberately not part of this:
  // a card can be due in a window the student has not studied in yet, and that is
  // not an empty dashboard — it is the next thing to do.
  const empty =
    totals !== undefined &&
    totals.minutes_studied === 0 &&
    totals.reviews === 0 &&
    totals.quizzes_taken === 0

  return (
    <div className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-6 px-5 md:px-[32px] py-8">
      <PageHeader
        // The window, in words, is the eyebrow: it is the one fact the heading
        // cannot carry, and every figure below is a count over these local days.
        eyebrow={t('dashboard.window', { from, to })}
        title={t('dashboard.title')}
        subtitle={t('dashboard.subtitle')}
      />

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
          <div className="card flex flex-col gap-4 p-6">
            {/* One column at phone width. A two-column grid whose cells are
                `min-width: auto` cannot shrink below their label's min-content, so at
                200% text "Minutes studied" and "Card reviews" forced the card — and
                with it the whole page — wider than the viewport. Measured: the page
                scrolled sideways at 390px in both themes before this. */}
            <dl className="grid min-w-0 grid-cols-1 gap-4 sm:grid-cols-2">
              {[
                { key: 'minutes', value: totals.minutes_studied },
                { key: 'reviews', value: totals.reviews },
                { key: 'cardsDue', value: totals.cards_due },
                { key: 'quizzes', value: totals.quizzes_taken },
              ].map((row) => (
                <div key={row.key} className="flex min-w-0 flex-col gap-1">
                  <dt className="label">{t(`dashboard.total.${row.key}`)}</dt>
                  <dd className="text-body">{row.value}</dd>
                </div>
              ))}
            </dl>
            {/* The sentence is the figures' equivalent and the section's summary: a
                reader who never looks at the numbers above still learns the one
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
        <Section title={t('dashboard.studyTime')}>
          <div className="card flex flex-col gap-4 p-6">
            <WidgetHeading title={t('dashboard.timePerCourse')} />
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
                <ul className="flex flex-col gap-3">
                  {courses.map((course) => (
                    /* A column at phone width, a row above `sm`. Three siblings
                       sharing one line — a course name, a bar and a count — cannot fit
                       at 390px with the text at 200%, and `truncate` on the name hid
                       the half that did not: a course the student cannot read is worse
                       than one that wraps. */
                    <li
                      key={course.workspace_id}
                      className="flex min-w-0 flex-col gap-1 sm:flex-row sm:items-center sm:gap-3"
                    >
                      <span className="min-w-0 break-words text-label sm:flex-1">
                        {course.title}
                      </span>
                      <span className="flex min-w-0 flex-1 items-center gap-3">
                        <Bar
                          share={course.minutes / busiestCourse}
                          label={t('dashboard.minutes', { count: course.minutes })}
                        />
                        <span className="meta shrink-0">
                          {t('dashboard.minutes', { count: course.minutes })}
                        </span>
                      </span>
                    </li>
                  ))}
                </ul>
              </>
            )}
          </div>

          <div className="card flex flex-col gap-4 p-6">
            <WidgetHeading title={t('dashboard.reviewActivity')} />
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
                <ul className="flex flex-col gap-3">
                  {reviews.map((day) => (
                    <li
                      key={day.local_date}
                      className="flex min-w-0 flex-col gap-1 sm:flex-row sm:items-center sm:gap-3"
                    >
                      <span className="min-w-0 text-label sm:flex-1">{day.local_date}</span>
                      <span className="flex min-w-0 flex-1 items-center gap-3">
                        <Bar
                          share={day.reviews / busiestDay}
                          label={t('dashboard.reviews', { count: day.reviews })}
                        />
                        <span className="meta shrink-0">
                          {t('dashboard.reviews', { count: day.reviews })}
                        </span>
                      </span>
                    </li>
                  ))}
                </ul>
              </>
            )}
          </div>
        </Section>
      )}

      {empty && <EmptyState title={t('dashboard.title')} reason={t('dashboard.empty')} />}

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
          <>
            <div className="card flex flex-col gap-4 p-6">
              {mastery.length === 0 ? (
                <p className="text-small text-muted">{t('dashboard.noMastery')}</p>
              ) : (
                <>
                  <p className="text-small text-muted">
                    {t('dashboard.masterySummary', { topics: mastery.length })}
                  </p>
                  <ul className="flex flex-col gap-3">
                    {mastery.map((topic) => (
                      <li key={topic.source_id} className="flex min-w-0 flex-col gap-1">
                        <span className="min-w-0 break-words text-label">{topic.title}</span>
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

            {weakTopics.length > 0 && (
              <div className="card flex flex-col gap-4 p-6">
                <WidgetHeading
                  title={t('dashboard.weakTopics')}
                  // A real count of the rows below, in mono. `WidgetHeading`
                  // omits it rather than zero-filling, and the card is not rendered
                  // at all when the list is empty.
                  meta={t('dashboard.weakTopicsMeta', { count: weakTopics.length })}
                />
                <p className="text-small text-muted">{t('dashboard.weakTopicsHint')}</p>
                <ul className="flex flex-col gap-3">
                  {weakTopics.map((topic) => (
                    <li
                      key={topic.source_id}
                      className="flex min-w-0 flex-wrap items-center gap-x-3 gap-y-1"
                    >
                      <span className="min-w-0 flex-1 break-words text-label">{topic.title}</span>
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
          </>
        )}
      </Section>
    </div>
  )
}
