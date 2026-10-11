import { useTranslation } from 'react-i18next'

import { PageHeader, Section } from '@/components/layout/Section'
import { EmptyState } from '@/components/ui/States'

/**
 * Today, the default home.
 *
 * Every section is an honest empty state rather than a mock. There is no data
 * layer yet (that arrives in Phase 1), and a screen of invented study blocks and
 * due counts would be the worst kind of slop here: it would look like the
 * planner was working. Each state names what fills it instead.
 *
 * The four sections are the ones the planner and the study tools actually
 * produce, so the layout is real and will fill in as those phases land.
 */
export function TodayScreen() {
  const { t } = useTranslation()

  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-8 px-5 py-8">
      <PageHeader title={t('nav.today')} subtitle={t('today.subtitle')} />

      <Section title={t('today.studyBlocks.title')}>
        <div className="card">
          <EmptyState
            title={t('today.studyBlocks.emptyTitle')}
            reason={t('today.studyBlocks.emptyReason')}
          />
        </div>
      </Section>

      <Section title={t('today.dueCards.title')}>
        <div className="card">
          <EmptyState
            title={t('today.dueCards.emptyTitle')}
            reason={t('today.dueCards.emptyReason')}
          />
        </div>
      </Section>

      <Section title={t('nav.tasks')}>
        <div className="card">
          <EmptyState
            title={t('today.tasks.emptyTitle')}
            reason={t('today.tasks.emptyReason')}
          />
        </div>
      </Section>

      <Section title={t('today.exams.title')}>
        <div className="card">
          <EmptyState
            title={t('today.exams.emptyTitle')}
            reason={t('today.exams.emptyReason')}
          />
        </div>
      </Section>
    </div>
  )
}
