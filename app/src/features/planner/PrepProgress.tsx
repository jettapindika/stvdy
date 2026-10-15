import { useTranslation } from 'react-i18next'

import { useGoalProgress } from '@/api/queries/goals'
import { LoadingState } from '@/components/ui/States'

/**
 * A goal's prep progress: its named parts, and no composite.
 *
 * The spec's honesty rules forbid a "readiness" score and a grade prediction
 * (`SPEC.md:75-76`), so this card shows the components the server actually
 * computed — topic coverage, quiz accuracy over the last 14 days, and the cards
 * due — and each one is a number that traces to the student's own attempts. The
 * design's own Academic-patterns sheet draws a single "Mastery 32%" family; that
 * is the one thing in the design this card may not transcribe, and the design
 * record says so in § 6.
 *
 * **Nothing is invented while loading, and nothing is invented from nothing.**
 * `quiz_accuracy_pct` is `null`, never `0`, when the goal has no attempts: `0%` is
 * a claim the app cannot make from an empty set, so the card says "no quizzes yet"
 * instead. The same rule makes the coverage figure absent rather than zero for a
 * goal whose topics are not known yet.
 *
 * **The coverage fraction is counted from `topics`, not read from a field.** The
 * route sends the list the percentage is computed over, so `6 / 8 topics reviewed`
 * is `topics.filter(topic => topic.accuracy !== null).length` over `topics.length`
 * — the same rows the server's own `covered` count walks, and the reason the
 * fraction and the percentage cannot disagree.
 *
 * The percentage is repeated in the bar's own reading (`text-meta`, indigo) rather
 * than drawn as a bare track: the number is the fact, and the bar is its shape.
 */
export function PrepProgress({ goalId, title }: { goalId: string; title: string }) {
  const { t } = useTranslation()
  const progress = useGoalProgress(goalId)

  if (progress.isPending) return <LoadingState label={t('planner.prep.loading')} />

  const data = progress.data
  if (data === undefined) {
    // A refused or failed read shows the goal and no numbers. A card full of
    // zeroes would be claims the app cannot make.
    return (
      <div className="card flex flex-col gap-2 p-5">
        <h3 className="min-w-0 break-words">{t('planner.prep.title', { goal: title })}</h3>
        <p className="text-small text-muted">{t('planner.prep.unavailable')}</p>
      </div>
    )
  }

  const reviewed = data.topics.filter((topic) => topic.accuracy !== null).length

  return (
    <div className="card flex flex-col gap-5 p-5">
      <div className="flex flex-col gap-1">
        {/* The eyebrow names what the card is about rather than restating the
            heading: the goal is the heading, and this is the window the numbers
            are read over. */}
        <p className="eyebrow">{t('planner.prep.window', { days: data.window_days })}</p>
        <h3 className="min-w-0 break-words">{t('planner.prep.title', { goal: title })}</h3>
      </div>

      {data.topic_coverage_pct === null ? (
        <p className="text-small text-muted">{t('planner.prep.noTopics')}</p>
      ) : (
        <div className="flex flex-col gap-2">
          <div className="flex flex-wrap items-baseline justify-between gap-2">
            <p className="min-w-0 wrap-anywhere text-label text-text">{t('planner.prep.coverage')}</p>
            <p className="meta min-w-0 wrap-anywhere text-primary-text">
              {t('planner.prep.percent', { percent: data.topic_coverage_pct })}
            </p>
          </div>
          <p className="min-w-0 wrap-anywhere text-small text-text">
            {t('planner.prep.topicsReviewed', { done: reviewed, total: data.topics.length })}
          </p>
          {/* The bar is decorative: the percentage above it is the fact, and the
              fraction beside it says what the percentage is of. */}
          <div aria-hidden="true" className="h-1.5 w-full overflow-hidden rounded-pill bg-surface-raised">
            <div
              className="h-full rounded-pill bg-primary"
              style={{ width: `${String(Math.min(100, Math.max(0, data.topic_coverage_pct)))}%` }}
            />
          </div>
          {/* The frame's own line, and it is the honesty rule in five words: a
              topic that has been touched is not a topic that has been learned. */}
          <p className="meta">{t('planner.prep.coverageNote')}</p>
        </div>
      )}

      <dl className="flex flex-wrap gap-x-4 gap-y-4">
        <div className="flex min-w-0 flex-col">
          <dt className="text-h2 min-w-0 break-words">
            {data.quiz_accuracy_pct === null
              ? t('planner.prep.noQuizzes')
              : t('planner.prep.percent', { percent: data.quiz_accuracy_pct })}
          </dt>
          <dd className="text-label text-muted">{t('planner.prep.accuracy')}</dd>
        </div>
        <div className="flex min-w-0 flex-col">
          <dt className="text-h2 min-w-0 break-words">{data.cards_due}</dt>
          <dd className="text-label text-muted">{t('planner.prep.cardsDue')}</dd>
        </div>
      </dl>

      {data.weak_topics.length === 0 ? (
        <p className="text-small text-muted">{t('planner.prep.noWeak')}</p>
      ) : (
        <div className="flex flex-col gap-2 border-t border-border pt-4">
          <p className="text-label text-text">{t('planner.prep.weak')}</p>
          {/* A list of topics rather than a highlighted chip: a weak topic is a
              fact about the student's attempts, and marking it in colour alone
              would be a state carried by colour (gate 5d). */}
          <ul className="flex flex-wrap gap-2">
            {data.weak_topics.map((topic) => (
              <li key={topic} className="chip wrap-anywhere">
                {topic}
              </li>
            ))}
          </ul>
        </div>
      )}
    </div>
  )
}
