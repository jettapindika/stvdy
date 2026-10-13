import { useTranslation } from 'react-i18next'

import { BillingScreen } from '@/features/billing/BillingScreen'
import { PlanUsageSections } from '@/features/billing/PlanUsageSections'
import { PageFooter, PageHeader } from '@/components/layout/Section'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE } from '@/lib/time'

/**
 * Plan and usage, on web and desktop.
 *
 * The body is `PlanUsageSections`, and below it `BillingScreen` — the purchase
 * half: the credit packs and the way to the plans. That split is the mechanism
 * rather than tidiness (A322). `/billing` chooses its element by platform at the
 * router, so the phone renders `MobilePlanScreen`, which composes
 * `PlanUsageSections` and a sentence saying the app does not sell subscriptions;
 * **this** element is the one that carries prices and buy buttons.
 *
 * `BillingScreen` is guarded by `__PURCHASE_UI__`, the same build-time constant the
 * router's purchase routes use, and that guard is what makes the store build's
 * bytes clean rather than merely its rendering: in the store build this is
 * `false ? <BillingScreen /> : null`, the branch is dropped, and with it the
 * imports that reach `POST /billing/checkout`. A `platform.kind()` check would
 * leave every one of those modules in the file.
 *
 * The footer names the zone the renewal and reset dates above were rendered in.
 * Both are instants the server computed for the account's own month boundary, and
 * a reader in another zone has no other way to know which day they name.
 */
export function PlanUsageScreen() {
  const { t } = useTranslation()
  const session = useSession()
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

  return (
    <div className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-6 px-5 py-8">
      <PageHeader title={t('billing.planAndUsage')} subtitle={t('plan.subtitle')} />
      <PlanUsageSections />
      {__PURCHASE_UI__ ? <BillingScreen /> : null}
      <PageFooter left={t('plan.footer.timezone', { zone: timezone })} />
    </div>
  )
}
