import { useTranslation } from 'react-i18next'

import { useGoalProgress } from '@/api/queries/goals'
import { LoadingState } from '@/components/ui/States'

/**
 * A goal's prep progress: four honest numbers, and no composite.
 *
 * The spec's honesty rules forbid a "readiness" score and a grade prediction, so
 * this card shows the components the server actually computed — topic coverage,
 * quiz accuracy over the last 14 days, the cards due, and which topics are weak —
 * and each one is a number that traces to the student's own attempts.
 *
 * **Nothing is invented while loading, and nothing is invented from nothing.**
 * `quiz_accuracy_pct` is `null`, never `0`, when the goal has no attempts: `0%` is
 * a claim the app cannot make from an empty set, so the card says "no quizzes yet"
 * instead. The same rule makes the coverage percentage absent rather than zero for
 * a goal whose topics are not known yet.
 */
export function PrepProgress({ goalId, title }: { goalId: string; title: string }) {
  const { t } = useTranslation()
  const progress = useGoalProgress(goalId)

  if (progress.isPending) return <LoadingState label={t('planner.prep.loading')} />

  const data = progress.data
  if (data === undefined) {
    // A refused or failed read shows the goal and no numbers. A card full of
    // zeroes would be four claims the app cannot make.
    return <p className="text-small text-muted">{t('planner.prep.unavailable')}</p>
  }

  return (
    <div className="raised flex flex-col gap-2 p-3">
      <p className="label">{t('planner.prep.title', { goal: title })}</p>

      <dl className="flex flex-wrap gap-x-6 gap-y-2">
        <div className="flex flex-col">
          <dt className="meta">{t('planner.prep.coverage')}</dt>
          <dd className="text-body text-text">
            {data.topic_coverage_pct === null
              ? t('planner.prep.noTopics')
              : t('planner.prep.percent', { percent: data.topic_coverage_pct })}
          </dd>
        </div>
        <div className="flex flex-col">
          <dt className="meta">{t('planner.prep.accuracy')}</dt>
          <dd className="text-body text-text">
            {data.quiz_accuracy_pct === null
              ? t('planner.prep.noQuizzes')
              : t('planner.prep.percent', { percent: data.quiz_accuracy_pct })}
          </dd>
        </div>
        <div className="flex flex-col">
          <dt className="meta">{t('planner.prep.cardsDue')}</dt>
          <dd className="text-body text-text">{data.cards_due}</dd>
        </div>
      </dl>

      {data.weak_topics.length === 0 ? (
        <p className="text-small text-muted">{t('planner.prep.noWeak')}</p>
      ) : (
        <div className="flex flex-col gap-1">
          <p className="meta">{t('planner.prep.weak')}</p>
          {/* A list of topics rather than a highlighted chip: a weak topic is a
              fact about the student's attempts, and marking it in colour alone
              would be a state carried by colour (gate 5(d)). */}
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
