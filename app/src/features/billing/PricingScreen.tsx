import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router'

import { asApiError } from '@/api/errors'
import { useCheckout, usePlans, type PlanOffer } from '@/api/queries/billing'
import { PageHeader, Section } from '@/components/layout/Section'
import { ApiErrorNotice, SkeletonRows } from '@/components/ui/States'
import { formatCount } from '@/features/billing/CreditMeter'
import { formatAmount } from '@/features/billing/checkout'
import { policyFor } from '@/features/billing/policy'
import { platform } from '@/platform'

/**
 * The pricing page: what each plan costs and what it includes.
 *
 * **Every number is the server's**, and that is the load-bearing property of this
 * screen rather than a nicety. `GET /plans` builds its prices from the same
 * `config/plans.toml` the checkout charges from, and its `student_price_idr_*`
 * fields are computed by `billing::price` — the one function that decides the
 * amount, including A298's rule that the student promo and a discount code do not
 * stack and the larger percentage wins. So a price on this page and the charge on
 * the checkout page cannot disagree, because there is one implementation between
 * them and it is the server's. The client's own reader of the matrix
 * (`features/billing/plans.ts`) is used by the tests to *assert* that, never to
 * render.
 *
 * **The page renders no purchase surface where the policy forbids one.** On a
 * phone `allowPurchaseUi` is false, and this screen then states the fact — the
 * same sentence `MobilePlanScreen` uses — and offers nothing: no price, no button,
 * no link. That is a store rule (A24) and the second lock: the route is not
 * reachable from the phone's navigation, and if it were reached it would still
 * sell nothing.
 *
 * **Yearly is the default**, because the yearly price is the one the matrix makes
 * cheaper per month and the toggle is a comparison a student should be able to
 * make; the two prices are both shown rather than the saving being computed, since
 * a saving this app calculated would be a number the server did not send.
 */

/** The limit axes the comparison table lists, in the order a student reads them. */
const COMPARISON: string[] = [
  'workspaces',
  'sources_total',
  'storage_mb',
  'devices_max',
  'credits_per_month',
  'reports_per_month',
  'flashcards_per_month',
  'quizzes_per_month',
  'google_accounts_max',
]

/** The FAQ, as keys. Each is a question and an answer in both locales. */
const FAQ: string[] = ['cancel', 'payment', 'mobile', 'student']

export function PricingScreen() {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const plans = usePlans()
  const checkout = useCheckout()
  const [yearly, setYearly] = useState(true)

  const policy = policyFor(platform.kind())
  const data = plans.data
  const error = asApiError(plans.error)
  const checkoutError = asApiError(checkout.error)

  // The same three readings as `BillingScreen`, and the policy one is checked
  // first because it is the one that must hold in the shipped bytes.
  const sellable = policy.allowPurchaseUi && policy.showPrices && data?.purchase_allowed === true

  const choose = (plan: PlanOffer) => {
    checkout.mutate(
      { plan_id: plan.id, billing_period: yearly ? 'yearly' : 'monthly' },
      { onSuccess: (payment) => void navigate(`/billing/checkout/${payment.id}`) },
    )
  }

  /** One plan's price for the selected period, or null when there is nothing to show. */
  const priceOf = (plan: PlanOffer): number | null => {
    if (!sellable) return null
    const monthly = data?.student_verified
      ? plan.student_price_idr_monthly
      : plan.price_idr_monthly
    const annual = data?.student_verified ? plan.student_price_idr_yearly : plan.price_idr_yearly
    const chosen = yearly ? annual : monthly
    return chosen ?? null
  }

  /** One limit value as a student reads it: a number, or the word for uncapped. */
  const limitOf = (plan: PlanOffer, axis: string): string => {
    const value = plan.limits[axis]
    if (value === 'unlimited') return t('plan.limit.unlimited')
    if (typeof value === 'number') return formatCount(value, i18n.language)
    // An axis the payload does not carry, or one that is not a limit at all: no
    // number rather than a zero.
    return '—'
  }

  if (!policy.allowPurchaseUi) {
    return (
      <div className="flex flex-col gap-6 px-5 py-6">
        <PageHeader title={t('pricing.title')} subtitle={t('plan.subtitle')} />
        <p className="max-w-prose text-sm text-muted">{t('plan.mobile.noPurchase')}</p>
      </div>
    )
  }

  return (
    <div className="flex flex-col gap-6 px-5 py-6">
      <PageHeader title={t('pricing.title')} subtitle={t('pricing.subtitle')} />

      {plans.isPending && <SkeletonRows rows={3} label={t('pricing.loading')} />}
      {error !== null && <ApiErrorNotice error={error} />}
      {checkoutError !== null && <ApiErrorNotice error={checkoutError} />}

      {data !== undefined && (
        <>
          {/* The student banner, shown only to a verified student and only where
              the matrix grants a discount at all. The percentage is the matrix's
              own `student_percent_off`, and the price it produces is the server's
              `student_price_idr_*` — so the banner cannot promise a discount the
              checkout would not apply. */}
          {data.student_verified &&
            data.plans.some((plan) => plan.student_percent_off > 0) && (
              <p className="note note-success text-sm">
                {t('pricing.studentBanner', {
                  percent: Math.max(...data.plans.map((plan) => plan.student_percent_off)),
                })}
              </p>
            )}

          <div className="flex flex-wrap items-center gap-3">
            <button
              type="button"
              className="btn btn-secondary min-h-11"
              aria-pressed={yearly}
              onClick={() => setYearly((wasYearly) => !wasYearly)}
            >
              {yearly ? t('pricing.yearly') : t('pricing.monthly')}
            </button>
            <p className="text-sm text-muted">
              {yearly ? t('pricing.yearlyHint') : t('pricing.monthlyHint')}
            </p>
          </div>

          <ul className="grid gap-4 sm:grid-cols-2 lg:grid-cols-3">
            {data.plans.map((plan) => {
              const price = priceOf(plan)
              // A price of zero is the matrix saying "this costs nothing", which is
              // the free plan — and `formatAmount(0)` would render "Rp 0", a price
              // where the honest answer is the word. `null` is different again: the
              // policy is hiding prices, so there is nothing to say at all.
              const free = price === 0
              const isCurrent = plan.id === data.current_plan
              return (
                <li key={plan.id} className="card flex flex-col gap-3 p-4">
                  <div className="flex items-baseline justify-between gap-2">
                    <h2 className="text-h3">{plan.name}</h2>
                    {/* The word, not only the colour: which plan the account is
                        on is a state, and gate 5d forbids colour alone. */}
                    {isCurrent && <span className="chip chip-working">{t('pricing.current')}</span>}
                  </div>

                  {price !== null && !free && (
                    <p className="text-h2">
                      {formatAmount(price, i18n.language)}
                      <span className="text-sm text-muted">
                        {yearly ? t('pricing.perYear') : t('pricing.perMonth')}
                      </span>
                    </p>
                  )}
                  {(price === null || free) && (
                    <p className="text-sm text-muted">{t('pricing.free')}</p>
                  )}

                  {/* No line when the payload carries no allowance, rather than a
                      `0` standing in for "not shown". The count goes to `t()` as a
                      **number**: i18next resolves `_one`/`_other` from a numeric
                      `count`, and a pre-formatted string silently renders the key
                      itself — which is why the number formatter is in the locale
                      string (`{{count, number}}`) rather than in a call here. */}
                  {plan.credits_per_month !== null && (
                    <p className="text-sm text-muted">
                      {t('pricing.creditsIncluded', { count: plan.credits_per_month })}
                    </p>
                  )}

                  {/* No button on the free plan: there is nothing to buy, and a
                      disabled control that says "choose" is a promise the matrix
                      does not make. Nor on the plan already in effect. */}
                  {price !== null && price > 0 && !isCurrent && (
                    <button
                      type="button"
                      className="btn btn-primary min-h-11"
                      disabled={checkout.isPending}
                      onClick={() => choose(plan)}
                    >
                      {t('pricing.choose', { plan: plan.name })}
                    </button>
                  )}
                </li>
              )
            })}
          </ul>

          <Section title={t('pricing.comparison')}>
            {/* A table, not a grid of cards: the comparison is read across plans
                for one axis, and a table is what a screen reader can navigate by
                row and column. The scroll container is for the phone width, where
                three plan columns do not fit. */}
            <div className="overflow-x-auto">
              <table className="w-full min-w-[32rem] text-sm">
                <caption className="sr-only">{t('pricing.comparisonCaption')}</caption>
                <thead>
                  <tr className="border-b border-border text-left">
                    <th scope="col" className="label py-2 pr-4">
                      {t('pricing.axis')}
                    </th>
                    {data.plans.map((plan) => (
                      <th key={plan.id} scope="col" className="label py-2 pr-4">
                        {plan.name}
                      </th>
                    ))}
                  </tr>
                </thead>
                <tbody>
                  {COMPARISON.map((axis) => (
                    <tr key={axis} className="border-b border-border">
                      <th scope="row" className="py-2 pr-4 font-normal text-muted">
                        {/* The wire spells the axis `sources_total`; the limit rows'
                            keys spell it `sourcesTotal` (`PlanUsageSections` reads
                            the same ones). Converting here rather than keeping a
                            second list of key names is what stops a row the locale
                            files have from going missing from this table. */}
                        {t(`plan.limit.${axis.replace(/_([a-z])/g, (_, letter: string) => letter.toUpperCase())}`)}
                      </th>
                      {data.plans.map((plan) => (
                        <td key={plan.id} className="meta py-2 pr-4">
                          {limitOf(plan, axis)}
                        </td>
                      ))}
                    </tr>
                  ))}
                </tbody>
              </table>
            </div>
          </Section>

          <Section title={t('pricing.faq')}>
            <dl className="flex flex-col gap-3">
              {FAQ.map((entry) => (
                <div key={entry} className="flex flex-col gap-1">
                  <dt className="text-sm font-medium">{t(`pricing.faqItems.${entry}.q`)}</dt>
                  <dd className="max-w-prose text-sm text-muted">
                    {t(`pricing.faqItems.${entry}.a`)}
                  </dd>
                </div>
              ))}
            </dl>
          </Section>
        </>
      )}
    </div>
  )
}
