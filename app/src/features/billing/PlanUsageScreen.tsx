import { useTranslation } from 'react-i18next'

import { PageHeader } from '@/components/layout/Section'
import { PlanUsageSections } from '@/features/billing/PlanUsageSections'

/**
 * Plan and usage, on web and desktop.
 *
 * The body is `PlanUsageSections`; what is here is the header and the fact that
 * this is the *desktop* route element. `/billing` chooses between this and
 * `MobilePlanScreen` by platform at the router (A322), so whatever purchase
 * surface Phase 13 adds to the web/desktop plan screen is unreachable from a
 * phone build rather than merely hidden by a policy check inside a component.
 *
 * `config/billing_policy.toml` still governs the one gated thing this screen
 * shares with the phone: the export formats the plan does not include are
 * `LockedFeature`, which reads that policy itself, so on iOS and Android it
 * states the fact with no price, no button and no link (A143).
 */
export function PlanUsageScreen() {
  const { t } = useTranslation()

  return (
    <div className="flex flex-col gap-6 px-5 py-6">
      <PageHeader title={t('billing.planAndUsage')} subtitle={t('plan.subtitle')} />
      <PlanUsageSections />
    </div>
  )
}
