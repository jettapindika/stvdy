import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router'

import { asApiError } from '@/api/errors'
import { useCheckout, useInvoices, usePlans } from '@/api/queries/billing'
import { Section } from '@/components/layout/Section'
import { ApiErrorNotice, EmptyState, SkeletonRows } from '@/components/ui/States'
import { formatCount } from '@/features/billing/CreditMeter'
import { formatAmount } from '@/features/billing/checkout'
import { policyFor } from '@/features/billing/policy'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { useSession } from '@/features/auth/useSession'
import { platform } from '@/platform'

/**
 * The billing page's purchase half: credit packs, and the way to the plans.
 *
 * **This is the web/desktop element's own content, and the phone never renders
 * it.** `/billing` picks its element by `platform.kind()` (A322): iOS and Android
 * get `MobilePlanScreen`, which composes `PlanUsageSections` and a sentence saying
 * the app does not sell subscriptions. So the absence of a purchase surface on a
 * phone is a property of the route table rather than a check inside this
 * component — and this component is where the prices and the buy buttons live,
 * which is exactly what a store review must not find in a phone build.
 *
 * The plan, the renewal date and the credits are **not** here: they are
 * `PlanUsageSections`, which both elements share, and they come from `GET /usage`.
 * The split is deliberate — one copy of the account's numbers, and one place the
 * purchase surface can be added without the phone inheriting it.
 *
 * **Every price is the server's.** `GET /plans` carries `credit_packs[].price_idr`
 * from the same `config/plans.toml` the checkout charges from, and it is absent
 * where the platform's policy hides prices — which is why a pack without a price
 * renders no button rather than a guessed number. Nothing here computes a total:
 * the amount a student is charged is minted by `POST /billing/checkout` and shown
 * on the checkout screen from `payments.amount_idr`.
 *
 * The client's `policyFor` is checked as well as the server's `purchase_allowed`,
 * and the two are not redundant: the server flag is the *enforcement* (a patched
 * client still gets 403), and the policy file is what keeps the shipped bytes from
 * containing a purchase path at all. They are two readings of one config file, and
 * the bundle check (`scripts/check-no-purchase-ui.mjs`) is what proves the second.
 */
export function BillingScreen() {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const plans = usePlans()
  const invoices = useInvoices()
  const checkout = useCheckout()
  const session = useSession()
  const policy = policyFor(platform.kind())

  const data = plans.data
  const error = asApiError(plans.error)
  const checkoutError = asApiError(checkout.error)
  const invoicesError = asApiError(invoices.error)

  // The account's zone, as everywhere else that renders an instant: a payment made
  // at 06:30 UTC on the 1st is 13:30 on the 1st in Jakarta, and shown in UTC it
  // would be the previous day.
  const timezone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

  /** True only when the policy, the server and the payload all allow a purchase. */
  const sellable = policy.allowPurchaseUi && policy.showPrices && data?.purchase_allowed === true

  const buy = (credits: number) => {
    checkout.mutate(
      { credit_pack: credits },
      { onSuccess: (payment) => void navigate(`/billing/checkout/${payment.id}`) },
    )
  }

  return (
    <div className="flex flex-col gap-6">
      <Section title={t('billing.buyCredits')}>
        {plans.isPending && <SkeletonRows rows={2} label={t('billing.packsLoading')} />}
        {error !== null && <ApiErrorNotice error={error} />}
        {checkoutError !== null && <ApiErrorNotice error={checkoutError} />}

        {data !== undefined && (
          <>
            <p className="max-w-prose text-sm text-muted">{t('billing.packsHint')}</p>
            <ul className="flex flex-col gap-2">
              {data.credit_packs.map((pack) => (
                <li
                  key={pack.credits}
                  className="card flex flex-wrap items-center justify-between gap-3 p-3"
                >
                  <div className="flex flex-col">
                    <p className="text-sm font-medium">
                      {t('billing.packCredits', { count: pack.credits })}
                    </p>
                    {/* No price where the policy hides them, and no `0` standing in
                        for one: the honest answer to "how much" is silence. */}
                    {sellable && pack.price_idr !== undefined && (
                      <p className="meta">{formatAmount(pack.price_idr, i18n.language)}</p>
                    )}
                  </div>
                  {sellable && pack.price_idr !== undefined && (
                    <button
                      type="button"
                      className="btn btn-secondary min-h-11"
                      // The pending flag is the mutation's, so a double tap
                      // cannot mint two invoices for one pack.
                      disabled={checkout.isPending}
                      onClick={() => buy(pack.credits)}
                    >
                      {t('billing.buyPack', { count: pack.credits })}
                    </button>
                  )}
                </li>
              ))}
            </ul>
            <p className="text-xs text-subtle">{t('billing.packsNeverExpire')}</p>
          </>
        )}
      </Section>

      {policy.allowPurchaseUi && (
        <Section title={t('billing.subscription')}>
          <p className="max-w-prose text-sm text-muted">{t('billing.subscriptionHint')}</p>
          <button
            type="button"
            className="btn btn-primary min-h-11 self-start"
            onClick={() => void navigate('/pricing')}
          >
            {t('billing.seePlans')}
          </button>
        </Section>
      )}

      {/* Payment history, on every platform. A phone shows it too: the list is a
          record of what the account already paid, which is not a purchase surface
          — there is no price to scan, no code and no way to buy from it. Hiding it
          on a phone would be hiding the account's own receipts. */}
      <Section title={t('billing.history')}>
        {invoices.isPending && <SkeletonRows rows={2} label={t('billing.historyLoading')} />}
        {invoicesError !== null && <ApiErrorNotice error={invoicesError} />}

        {invoices.data !== undefined && invoices.data.invoices.length === 0 && (
          <EmptyState
            title={t('billing.noPayments')}
            reason={t('billing.noPaymentsReason')}
            padded={false}
          />
        )}

        {invoices.data !== undefined && invoices.data.invoices.length > 0 && (
          <ul className="flex flex-col divide-y divide-border border-y border-border text-sm">
            {invoices.data.invoices.map((invoice) => (
              <li key={invoice.id} className="flex flex-wrap items-baseline gap-x-3 gap-y-1 py-2">
                {/* The status word, never its tone alone: `paid` and `review` are
                    the two a student is looking for, and gate 5d forbids a state
                    that exists only as a colour. */}
                <span className={STATUS_CHIP[invoice.status] ?? 'chip'}>
                  {t(`billing.status.${invoice.status}`, { defaultValue: invoice.status })}
                </span>
                <span className="text-muted">
                  {invoice.paid_at !== null
                    ? formatInZone(new Date(invoice.paid_at), timezone, { dateStyle: 'medium' })
                    : formatInZone(new Date(invoice.expires_at), timezone, { dateStyle: 'medium' })}
                </span>
                <span className="meta ml-auto">
                  {formatAmount(invoice.amount_idr, i18n.language)}
                </span>
                {/* What was bought, from the row's own fields: a pack by its
                    credits, a subscription by its plan and period. Nothing here
                    derives a label from the amount. */}
                <span className="w-full text-xs text-subtle">
                  {invoice.kind === 'credit_pack' && invoice.credits !== null
                    ? t('billing.historyPack', { count: invoice.credits })
                    : t('billing.historyPlan', {
                        plan: invoice.plan_id ?? '—',
                        period: t(`billing.period.${invoice.billing_period ?? 'monthly'}`),
                      })}
                </span>
              </li>
            ))}
          </ul>
        )}
      </Section>
    </div>
  )
}

/**
 * The tone of a payment's status chip.
 *
 * The word is what carries the state (`billing.status.<status>`); this only repeats
 * it, which is why an unknown status falls back to the neutral chip rather than to
 * a colour that would claim a meaning the status does not have.
 */
const STATUS_CHIP: Record<string, string> = {
  paid: 'chip chip-ready',
  pending: 'chip chip-working',
  expired: 'chip',
  failed: 'chip chip-failed',
  refunded: 'chip',
  review: 'chip chip-due',
}
