import { useTranslation } from 'react-i18next'

import { BillingScreen } from '@/features/billing/BillingScreen'
import { PlanUsageSections } from '@/features/billing/PlanUsageSections'
import { PageHeader } from '@/components/layout/Section'

/**
 * Plan and usage, on web and desktop.
 *
 * The body is `PlanUsageSections`, and below it `BillingScreen` — the purchase
 * half: the credit packs and the way to the plans. That split is the mechanism
 * rather than tidiness (A322). `/billing` chooses its element by platform at the
 * router, so the phone renders `MobilePlanScreen`, which composes
 * `PlanUsageSections` and a sentence saying the app does not sell subscriptions;
 * **this** element is the one that carries prices and buy buttons, and a phone
 * build therefore cannot reach them. `config/billing_policy.toml` is the second
 * lock, not the only one.
 *
 * The account's own numbers — the plan, the renewal date, the credits, the limits
 * and the costs — are one copy, in `PlanUsageSections`, shared by both elements, so
 * the phone and the desktop cannot drift into showing different numbers for the
 * same account.
 */
export function PlanUsageScreen() {
  const { t } = useTranslation()

  return (
    <div className="flex flex-col gap-6 px-5 py-6">
      <PageHeader title={t('billing.planAndUsage')} subtitle={t('plan.subtitle')} />
      <PlanUsageSections />
      <BillingScreen />
    </div>
  )
}
