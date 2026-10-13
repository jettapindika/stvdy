import { useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router'

import { asApiError } from '@/api/errors'
import { useCheckout, usePlans, type PlanOffer } from '@/api/queries/billing'
import { PageFooter, PageHeader, Section } from '@/components/layout/Section'
import { Icon } from '@/components/ui/Icon'
import { ApiErrorNotice, SkeletonRows } from '@/components/ui/States'
import { useSession } from '@/features/auth/useSession'
import { formatCount } from '@/features/billing/CreditMeter'
import { formatAmount } from '@/features/billing/checkout'
import { policyFor } from '@/features/billing/policy'
import { DEFAULT_TIMEZONE, zoneOffsetLabel } from '@/lib/time'
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
 *
 * **The account line carries no renewal date.** The frame draws one
 * (`RENEWS 12 NOV 2026`), but the period end is `GET /usage`'s, and this screen
 * reads `GET /plans` alone — a renewal the payload does not carry is omitted
 * rather than guessed at.
 */

/**
 * Every limit axis, named by the key the locale files give it.
 *
 * The wire spells the axes with underscores (`sources_total`) and the locale files
 * spell them in camelCase (`plan.limit.sourcesTotal`), and two of them do not map
 * mechanically: the wire's `storage_mb` is the locale's `plan.limit.storage`, which
 * a `_m` -> `M` substitution would render as the literal key `plan.limit.storageMb`.
 * So the mapping is written out rather than derived, and the order here is the
 * order the comparison table reads them in.
 *
 * `PlanUsageSections` renders the same keys from the same locale entries, which is
 * what keeps one axis from being named two ways on two screens.
 */
const AXIS_LABEL: Record<string, string> = {
  workspaces: 'plan.limit.workspaces',
  sources_total: 'plan.limit.sourcesTotal',
  storage_mb: 'plan.limit.storage',
  devices_max: 'plan.limit.devicesMax',
  credits_per_month: 'plan.limit.creditsPerMonth',
  reports_per_month: 'plan.limit.reportsPerMonth',
  flashcards_per_month: 'plan.limit.flashcardsPerMonth',
  quizzes_per_month: 'plan.limit.quizzesPerMonth',
  google_accounts_max: 'plan.limit.googleAccountsMax',
}

/** The limit axes the comparison table lists, in the order a student reads them. */
const COMPARISON: string[] = Object.keys(AXIS_LABEL)

/** The five axes a plan card lists as the features the plan includes. */
const FEATURES: string[] = [
  'credits_per_month',
  'workspaces',
  'sources_total',
  'storage_mb',
  'devices_max',
]

/** The FAQ, as keys. Each is a question and an answer in both locales. */
const FAQ: string[] = ['cancel', 'payment', 'mobile', 'student']

export function PricingScreen() {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const plans = usePlans()
  const checkout = useCheckout()
  const session = useSession()
  const [yearly, setYearly] = useState(true)

  const policy = policyFor(platform.kind())
  const data = plans.data
  const error = asApiError(plans.error)
  const checkoutError = asApiError(checkout.error)

  // The account's zone, as everywhere the app states where "now" is: a reader in
  // Jakarta is not on UTC, and the footer is the one place this page can say so.
  const timeZone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

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
      <div className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-6 px-5 py-8">
        <PageHeader title={t('pricing.title')} subtitle={t('plan.subtitle')} />
        <p className="max-w-prose text-body text-muted">{t('plan.mobile.noPurchase')}</p>
      </div>
    )
  }

  // The plan in effect, named from the payload's own `current_plan`. Absent from
  // the matrix is possible (a stale client, a plan retired between requests), and
  // then the line is omitted rather than naming a plan the server did not send.
  const currentPlan = data?.plans.find((plan) => plan.id === data.current_plan)

  return (
    <div className="mx-auto flex w-full min-w-0 max-w-3xl flex-col gap-6 px-5 py-8">
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
              <p className="note note-success text-label">
                {t('pricing.studentBanner', {
                  percent: Math.max(...data.plans.map((plan) => plan.student_percent_off)),
                })}
              </p>
            )}

          {/* The interval row: the account's plan on the left, the period the
              prices are quoted for on the right. The renewal date the frame draws
              is `GET /usage`'s and this screen does not read it, so the line is
              omitted rather than filled with a date the payload did not send. */}
          <div className="flex flex-wrap items-center justify-between gap-4">
            <div className="flex min-w-0 flex-col gap-1">
              {currentPlan !== undefined && (
                <p className="text-label font-medium">
                  {t('pricing.accountPlan', { plan: currentPlan.name })}
                </p>
              )}
            </div>
            <div className="flex flex-wrap items-center gap-3">
              <button
                type="button"
                className="btn btn-secondary min-h-11"
                aria-pressed={yearly}
                onClick={() => setYearly((wasYearly) => !wasYearly)}
              >
                {yearly ? t('pricing.yearly') : t('pricing.monthly')}
              </button>
              <p className="text-small text-muted">
                {yearly ? t('pricing.yearlyHint') : t('pricing.monthlyHint')}
              </p>
            </div>
          </div>

          {/* `auto-fit` with a **rem** floor, not `lg:grid-cols-3`. A fixed three-column
              grid leaves each card 160px at a 32px root, and the price renders at the
              48px h2 size — so `IDR 125,000 per year` measured 179px in a 160px box and
              was clipped. A rem floor grows with the root, so the grid drops to two
              columns (and then one) as the reader enlarges their text, which a fixed
              column count cannot do. At 100% the arithmetic still yields three: 21rem is
              336px, and three of those fit the content column. */}
          <ul className="grid gap-5 sm:grid-cols-2 lg:grid-cols-[repeat(auto-fit,minmax(21rem,1fr))]">
            {data.plans.map((plan) => {
              const price = priceOf(plan)
              // A price of zero is the matrix saying "this costs nothing", which is
              // the free plan — and `formatAmount(0)` would render "Rp 0", a price
              // where the honest answer is the word. `null` is different again: the
              // policy is hiding prices, so there is nothing to say at all.
              const free = price === 0
              const isCurrent = plan.id === data.current_plan
              // The amount and its currency, as the two pieces `formatAmount`
              // already keeps apart with a no-break space.
              const [currency, digits] = (price === null
                ? ''
                : formatAmount(price, i18n.language)
              ).split('\u00a0')
              return (
                <li key={plan.id} className="card flex min-w-0 flex-col gap-4 p-6">
                  <div className="flex flex-wrap items-baseline justify-between gap-2">
                    {/* The frame's plan name is 24/32 at weight 600, which is what
                        the base `h2` rule already sets. */}
                    <h2>{plan.name}</h2>
                    {/* The word, not only the colour: which plan the account is
                        on is a state, and gate 5d forbids colour alone. */}
                    {isCurrent && <span className="chip chip-working">{t('pricing.current')}</span>}
                  </div>

                  {price !== null && !free && (
                    /* The price is a currency and a number, and at 200% text the two
                       together are wider than the card: `--text-h2` is `1.5rem`, so
                       on a 1365px window it renders at 48px and `IDR 125,000`
                       measures wider than the three-column grid leaves it, so the
                       figure the student is deciding about was cut off.

                       `formatAmount` joins the two with a **no-break space**, which
                       is right at 100% — the currency and its number are one price
                       and must not drift apart — but it is precisely what forbids
                       the break a 200% reader needs. `<wbr>` puts the opportunity
                       back without touching the string: the break falls after the
                       currency, the digits stay whole, and `textContent` still
                       carries the same `IDR\u00a0…` the tests match on.

                       `overflow-wrap: anywhere` is deliberately **not** used here,
                       though it is the right rule for the workspace panels. There,
                       a long Indonesian word has no break opportunity at all; here
                       it would split the digits, and `IDR 125,0` / `00` is a price
                       a student misreads — worse than the clip it replaces. */
                    <p className="text-h2">
                      {currency}
                      {digits !== undefined && (
                        <>
                          {'\u00a0'}
                          <wbr />
                          {digits}
                        </>
                      )}
                      <span className="text-small text-muted">
                        {yearly ? t('pricing.perYear') : t('pricing.perMonth')}
                      </span>
                    </p>
                  )}
                  {free && <p className="text-h2">{t('pricing.free')}</p>}
                  {price === null && (
                    <p className="text-h2 text-muted">{t('pricing.priceHidden')}</p>
                  )}

                  {/* The currency and the period, both from the payload's own
                      fields — the word for the period comes from the same
                      `billing.period.*` keys the payment history uses, and the
                      currency from `GET /plans`' `currency`. */}
                  <p className="meta uppercase">
                    {free
                      ? t('pricing.noPaymentRequired')
                      : t('pricing.billedInterval', {
                          period: t(yearly ? 'billing.period.yearly' : 'billing.period.monthly'),
                          currency: data.currency,
                        })}
                  </p>

                  <div className="border-t border-border" />

                  {/* What the plan includes, as the plan's own limits: the frame's
                      feature list, rendered from the payload rather than from copy
                      this app would have to invent. */}
                  <ul className="flex min-w-0 flex-col gap-4">
                    {FEATURES.map((axis) => (
                      /* **The row wraps, and it must not be given `min-w-0`.** This is
                         the one place the two fixes look alike and are opposites.

                         A flex item's `min-width: auto` is its min-content, so at a
                         32px root the longest feature name ("Workspaces" in
                         `text-label`, 28px) is 167px of glyphs that refuse to shrink.
                         With the value span's 118.8px and two 24px gaps the row's
                         min-content was 353.6px, the card's 449.6, and the grid track
                         — `auto`, so floored at its items' min-content — resolved to
                         452 inside a 310px column: measured in Chromium at 390px,
                         `ul.grid.gap-5` had `scrollWidth` 452 against a `clientWidth`
                         of 310 and the page scrolled sideways to 492, both themes, at
                         200% only.

                         `min-w-0` removes that floor, and on its own it trades one
                         defect for a worse one: with the floor gone the row fits on one
                         line and the label is squeezed into what is left — **25px**
                         for "Workspaces", measured, against the 167px the word needs.
                         A feature name rendered one character per line is not a fix.
                         So the row keeps its min-content floor and **wraps**: the label
                         takes its own line at phone width and the value moves under it,
                         which is what `flex-wrap` does when the two cannot share a line.
                         `break-words` is the second half — the word may exceed the line
                         on its own, and `break-word` lets it break there, which is
                         enough because the container is now the full column. */
                      <li key={axis} className="flex flex-wrap items-center gap-x-3 gap-y-1">
                        <Icon name="check" className="shrink-0" />
                        <span className="break-words text-label font-medium">
                          {t(AXIS_LABEL[axis] ?? axis)}
                        </span>
                        <span className="meta ml-auto text-right">{limitOf(plan, axis)}</span>
                      </li>
                    ))}
                  </ul>

                  {/* No button on the free plan: there is nothing to buy, and a
                      disabled control that says "choose" is a promise the matrix
                      does not make. Nor on the plan already in effect. */}
                  {price !== null && price > 0 && !isCurrent && (
                    <button
                      type="button"
                      className="btn btn-primary mt-auto min-h-11"
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
              <table className="w-full min-w-[32rem] text-small">
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
                        {t(AXIS_LABEL[axis] ?? axis)}
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
                  <dt className="text-label font-medium">{t(`pricing.faqItems.${entry}.q`)}</dt>
                  <dd className="max-w-prose text-label text-muted">
                    {t(`pricing.faqItems.${entry}.a`)}
                  </dd>
                </div>
              ))}
            </dl>
          </Section>

          {/* Where "now" is, and what the money is in: both facts the app has, and
              the frame's own footer. */}
          <PageFooter
            left={`${timeZone} · ${zoneOffsetLabel(new Date(), timeZone)}`}
            right={t('pricing.currencyNote', { currency: data.currency })}
          />
        </>
      )}
    </div>
  )
}
