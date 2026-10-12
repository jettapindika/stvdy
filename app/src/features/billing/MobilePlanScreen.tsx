import { useTranslation } from 'react-i18next'

import { PageHeader } from '@/components/layout/Section'
import { PlanUsageSections } from '@/features/billing/PlanUsageSections'

/**
 * Plan and usage, on iOS and Android: the read-only plan screen.
 *
 * It shows the plan the account is on, the credits the app has and the usage the
 * server computes, and it offers **nothing to buy**: no price, no upgrade button,
 * no link, and no route to a checkout. That is a store rule rather than a design
 * choice — an app that points at an off-store purchase path is rejected — so the
 * absence is enforced twice over, and neither enforcement is this component:
 *
 * - `config/billing_policy.toml` turns off `allow_purchase_ui`, `show_prices` and
 *   `allow_purchase_cta_in_locked_messages` for `ios` and `android`, which is what
 *   makes `LockedFeature` render `lock.notOnPlan` here instead of naming a plan and
 *   linking to `/billing`;
 * - `scripts/check-no-purchase-ui.mjs` reads the shipped bytes and fails on a price,
 *   the checkout route or a purchase call. Reading this component is not the proof;
 *   the artifact is.
 *
 * **This is a separate route element from `PlanUsageScreen`, and that is the
 * mechanism, not a duplication** (A322). The router picks between them by
 * `platform.kind()`, so a purchase surface added to the web/desktop screen is not
 * reachable from the phone. The body they share is `PlanUsageSections` — one copy
 * of the plan, the credits, the limits and the costs, so the two screens cannot
 * drift into showing different numbers for the same account.
 *
 * The one string this screen adds is the honest statement of the rule itself, in
 * the words a reader would use: the app does not sell subscriptions. It names no
 * destination and asks for no action, because a sentence telling the user where to
 * go and buy is the same violation as a button that takes them there.
 */
export function MobilePlanScreen() {
  const { t } = useTranslation()

  return (
    <div className="flex flex-col gap-6 px-5 py-6">
      <PageHeader title={t('billing.planAndUsage')} subtitle={t('plan.subtitle')} />
      <PlanUsageSections />
      <p className="text-sm text-muted">{t('plan.mobile.noPurchase')}</p>
    </div>
  )
}
