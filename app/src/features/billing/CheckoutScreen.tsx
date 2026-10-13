import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useNavigate } from 'react-router'

import { asApiError } from '@/api/errors'
import { useCheckout, usePayment } from '@/api/queries/billing'
import { PageHeader } from '@/components/layout/Section'
import { Icon } from '@/components/ui/Icon'
import { ApiErrorNotice, SkeletonRows } from '@/components/ui/States'
import { checkoutState, clock, countdown, orderLines, type CheckoutState } from '@/features/billing/checkout'
import { CheckoutStatus } from '@/features/billing/CheckoutStatus'
import { PLAN_CURRENCY } from '@/features/billing/plans'
import { policyFor } from '@/features/billing/policy'
import { qrModules } from '@/features/billing/qrcode'
import { platform } from '@/platform'

/**
 * The QRIS checkout screen: the amount, the code, and what the server says.
 *
 * Five rules live here, and each is a rule rather than a step.
 *
 *  * **The total is the server's.** The order summary's amount is `orderLines(view)`'s
 *    own total, which reads `amount_idr`, `amount_before_discount_idr`,
 *    `discount_percent` and `discount_code` off the payment row. Nothing here adds
 *    or subtracts: A298 makes the student promo and a discount code one decision,
 *    made once by `billing::price`, and a client that re-derived it would be a
 *    second implementation of the money rule.
 *  * **The countdown is the server's `expires_at` against this client's clock.**
 *    `checkout.ts` takes `now` as a parameter and reads no clock, so the ticking is
 *    here and the arithmetic is there. The server re-checks expiry on every poll,
 *    so a client whose clock is wrong cannot extend its own invoice. The window's
 *    own *length* is not carried by the payload, so the row names the window and
 *    shows the server's remaining time rather than printing a 15 the app assumed.
 *  * **The QR is rendered only while the code is live.** A paid payment never
 *    shows a code again — a second scan of a dead code is how a student pays
 *    twice — and the state machine decides that, not this component.
 *  * **A request that did not reach the server is `offline`, not `failed`.** The
 *    code may still be valid, and telling a student their payment failed because
 *    their Wi-Fi dropped is a lie they might act on.
 *  * **The screen is a route, not a modal.** The frame draws a 640px dialog, but
 *    this is `/billing/checkout/:paymentId`, so it takes the modal's content idiom
 *    at `max-w-2xl` — a screen that owns its width says so itself.
 *
 * The screen polls through `usePayment`, which is what recovers a lost webhook
 * (A301): the server applies the same handler the webhook calls when the gateway
 * says paid, so the state moves without the student doing anything.
 *
 * **The payment id arrives as a prop**, not through `useParams`, and the router's
 * `CheckoutRoute` is what reads the path. It is the same shape `BillingRoute` and
 * `SyllabusReviewRoute` use, and it is what makes the screen testable without a
 * route: a component that read the param itself could only be rendered through a
 * `<Route>` in a test, so its five states could not be reached one at a time.
 */

/** How long a tick is, in milliseconds. One second, because the label shows seconds. */
const TICK_MS = 1000

export function CheckoutScreen({ paymentId }: { paymentId: string }) {
  const { t, i18n } = useTranslation()
  const navigate = useNavigate()
  const payment = usePayment(paymentId)
  const checkout = useCheckout()
  const [now, setNow] = useState(() => Date.now())
  const policy = policyFor(platform.kind())

  const view = payment.data
  const state: CheckoutState =
    view === undefined
      ? asApiError(payment.error)?.code === 'network'
        ? 'offline'
        : 'waiting'
      : checkoutState(view, now)

  // The tick runs only while the code can still die: a paid or failed invoice has
  // nothing left to count down, and an interval that keeps firing on a finished
  // screen is a wake-up every second for no reason.
  useEffect(() => {
    if (state !== 'waiting') return
    const timer = setInterval(() => setNow(Date.now()), TICK_MS)
    return () => clearInterval(timer)
  }, [state])

  const error = asApiError(payment.error)
  // A network failure is the `offline` state, which has its own words and its own
  // remedy. Every other failure is the server's own answer and gets the notice
  // that names the code it sent.
  const notice = error !== null && error.code !== 'network' ? error : null
  const checkoutError = asApiError(checkout.error)

  const remaining = view === undefined ? null : countdown(view.expires_at, now)

  /**
   * Start over: the same plan (or pack) and period, a fresh invoice.
   *
   * The server's reuse rule (A307) answers with the pending invoice if one is
   * still open, so this cannot mint a second chargeable QR for a period the
   * student is already paying — and when the old one has expired it mints the new
   * code the `expired` state offers.
   */
  const startOver = () => {
    if (view === undefined) return
    checkout.mutate(
      view.kind === 'credit_pack' && view.credits !== null
        ? { credit_pack: view.credits }
        : {
            plan_id: view.plan_id ?? undefined,
            billing_period: (view.billing_period ?? undefined) as 'monthly' | 'yearly' | undefined,
          },
      { onSuccess: (next) => void navigate(`/billing/checkout/${next.id}`) },
    )
  }

  // The platform gate, first, and before the poll: on a platform whose policy
  // forbids purchase UI this screen renders the honest statement and **mounts
  // nothing else** — no query, no QR, no order summary, no way to pay. The route
  // is registered unconditionally (the table is evaluated at module scope, before
  // a platform exists), so this is the check that makes an unreachable-by-
  // navigation URL *stay* unreachable when a reviewer types it: a store build must
  // not present a purchase screen on any path, not merely lack a link to one.
  //
  // It is the same shape `PricingScreen` uses, deliberately: one convention for
  // "this platform does not sell", rather than a route-level gate here and a
  // component-level one there.
  if (!policy.allowPurchaseUi) {
    return (
      <div className="mx-auto flex w-full min-w-0 max-w-2xl flex-col gap-6 px-5 py-8">
        <PageHeader title={t('checkout.title')} subtitle={t('plan.subtitle')} />
        <p className="max-w-prose text-body text-muted">{t('plan.mobile.noPurchase')}</p>
      </div>
    )
  }

  // The order summary's own total, taken from `orderLines` rather than summed
  // here. `orderLines` always ends with the total, and finding it by its label key
  // is what keeps this screen from being a second place that decides what "total"
  // means.
  const lines = view === undefined ? [] : orderLines(view, i18n.language)
  const total = lines.find((line) => line.labelKey === 'total')

  // What was bought, from the row's own fields: a pack by its credits, a
  // subscription by its plan and period. The same two keys the payment history
  // uses, so the two screens cannot name the same purchase differently.
  const planLine =
    view === undefined
      ? ''
      : view.kind === 'credit_pack' && view.credits !== null
        ? t('billing.historyPack', { count: view.credits })
        : t('billing.historyPlan', {
            plan: view.plan_id ?? '—',
            period: t(`billing.period.${view.billing_period ?? 'monthly'}`),
          })

  // **The wrapping rule, in one place, because it inherits.** `wrap-anywhere` is
  // `overflow-wrap: anywhere`, and on this container it reaches every box the 200%
  // measurement found clipping: the order-summary rows and the reference below
  // them, and — through `CheckoutStatus` — the state's own title, which is another
  // file. On a 390×844 phone at 200% text the card's content box is 244px, and
  // three things in it were wider: the order id (`stvdy_01j0…`, a single
  // unbreakable token at 316px of glyphs), a row whose label's longest word is
  // 114px and whose value is one unbreakable `IDR\u00a012,345` at 160px (290px
  // together against 244), and "confirmation" at 219px inside the 200px the status
  // card leaves beside its icon.
  //
  // `anywhere` and not `break-words`: `break-word` lowers only a box's *content*
  // width, not its **min-content** size, and min-content is what these boxes were
  // failing on — the flex row could not shrink its items below 114px and 160px. It
  // is the same rule, for the same reason, that the workspace panels take.
  //
  // Money is the one thing it must not reach, and that is a second mechanism rather
  // than an exception to this one: `break-normal` on the value and the `<wbr />`
  // inside `LineValue`, below.
  return (
    <div className="mx-auto flex w-full min-w-0 max-w-2xl flex-col gap-6 px-5 py-8 wrap-anywhere">
      <PageHeader title={t('checkout.title')} subtitle={t('checkout.subtitle')} />

      {payment.isPending && <SkeletonRows rows={3} label={t('checkout.loading')} />}
      {notice !== null && <ApiErrorNotice error={notice} />}
      {checkoutError !== null && <ApiErrorNotice error={checkoutError} />}

      {view !== undefined && (
        <>
          {/* The order summary: the amount the student is about to pay, its
              itemisation, and the reference a student quotes to support. The
              amount is `orderLines`' own total — the same number the webhook
              checks the gateway's claim against (A294). */}
          <div className="raised flex flex-col gap-4 rounded-field p-5">
            <div className="flex flex-wrap items-start justify-between gap-4">
              <div className="flex min-w-0 flex-col gap-1">
                <p className="eyebrow">{t('checkout.orderSummary')}</p>
                <p className="text-label font-medium">{planLine}</p>
                <p className="meta">{t('checkout.reference', { orderId: view.order_id })}</p>
              </div>
              {total !== undefined && (
                <div className="flex min-w-0 flex-col items-end gap-1">
                  <p className="text-h2 font-semibold break-normal">
                    <LineValue value={total.value} />
                  </p>
                  {/* The currency is the matrix's, from the same config the server
                      prices in. A build whose config could not be read omits the
                      label rather than printing a bare "TOTAL ·". */}
                  {PLAN_CURRENCY !== null && (
                    <p className="meta uppercase">
                      {t('checkout.totalCurrency', { currency: PLAN_CURRENCY })}
                    </p>
                  )}
                </div>
              )}
            </div>

            <dl className="flex flex-col divide-y divide-border text-label">
              {lines.map((line) => {
                // A money value is the one string on this screen the container's
                // `anywhere` must not reach, so it takes `break-normal` back — and
                // that is a second mechanism, not an exception to the first. See
                // `LineValue`.
                const money = line.value.includes('\u00a0')
                return (
                  <div
                    key={line.labelKey}
                    className="flex items-baseline justify-between gap-4 py-1.5"
                  >
                    <dt className="text-muted">
                      {t(`checkout.line.${line.labelKey}`, line.params ?? {})}
                    </dt>
                    <dd className={money ? 'meta break-normal' : 'meta'}>
                      <LineValue value={line.value} />
                    </dd>
                  </div>
                )
              })}
            </dl>
          </div>

          {/* The code, while it is live. `state === 'waiting'` and not "has a
              `qr_string`": a row whose code has expired still carries the string,
              and rendering it would invite a scan of a dead code. */}
          {state === 'waiting' && view.qr_string !== null && (
            <QrCode payload={view.qr_string} label={t('checkout.qrLabel')} />
          )}

          {/* How long is left, from the server's own `expires_at`. The window's
              length is not a field the payload carries, so the right-hand label
              names the window rather than printing a 15 this app assumed — a
              duration the server never sent is exactly the invented number the
              honesty rule forbids. */}
          {state === 'waiting' && remaining !== null && (
            <div className="flex flex-wrap items-center justify-between gap-3">
              <p className="flex items-center gap-2 text-body text-warning">
                <Icon name="clock" className="shrink-0" />
                {t('checkout.expiresIn', { time: clock(remaining.mm, remaining.ss) })}
              </p>
              <p className="meta uppercase">{t('checkout.windowLabel')}</p>
            </div>
          )}
        </>
      )}

      {/* The state renders whenever there is one to report — including `offline`,
          which by definition has no `view`: the request never arrived. Gating this
          on the payload would render an empty screen for exactly the failure the
          state exists to describe, which is the defect `CheckoutScreen.test.tsx`'s
          offline case found. */}
      {(view !== undefined || state === 'offline') && (
        <CheckoutStatus
          state={state}
          onNewCode={startOver}
          onRetry={() => void payment.refetch()}
        />
      )}

      {view !== undefined && (
        <p className="text-caption text-muted">{t('checkout.guidance')}</p>
      )}
    </div>
  )
}

/**
 * An order-summary value, with a money figure kept whole.
 *
 * **This is the second mechanism, and it is deliberately not the first.** The card
 * takes `overflow-wrap: anywhere` because the order id and the row labels need a
 * break opportunity wherever it falls; a money figure is the one string that must
 * not have one, because `IDR 12,345` split as `IDR 12,3` / `45` is a price a
 * student misreads, and that is worse than the clip it replaced. An identifier and
 * a figure are both unbreakable tokens in a 244px box, and they are not the same
 * problem: an id may be cut anywhere, a figure may only be cut at its currency.
 *
 * So the value's own box takes `break-normal` back (restoring the default the
 * container's `anywhere` took away, and with it the digits' min-content width) and
 * `<wbr>` supplies the one break opportunity the figure does need — after the
 * currency, which is where `formatAmount` puts the no-break space it joins the two
 * with. The digits stay whole, and `textContent` still carries the same
 * `IDR\u00a0…` the money assertions in `CheckoutScreen.test.tsx` match on.
 *
 * This is the technique `PricingScreen` uses for the same figure at 200% text, for
 * the same reason; the two fixes look alike and are not alike, which is why each
 * file says which one it is.
 */
function LineValue({ value }: { value: string }) {
  const [currency, digits] = value.split('\u00a0')
  // No no-break space means no currency: a bare count (`formatCount`) or a
  // percentage has no unit a break could separate from its number, so it inherits
  // the container's `anywhere` and breaks like any other string.
  if (digits === undefined) return <>{value}</>
  return (
    <>
      {currency}
      {'\u00a0'}
      <wbr />
      {digits}
    </>
  )
}

/**
 * The QR, as an SVG built from the payload the server sent.
 *
 * **The payload is rendered verbatim.** `qr_string` is the gateway's own string
 * (or the mock's, under the sandbox gateway), and nothing here re-encodes,
 * re-formats or wraps it: a QR the app composed itself would be a code the gateway
 * does not recognise, and the student would scan it and pay nothing.
 *
 * Two things are load-bearing:
 *
 *  * **The modules are drawn with `currentColor` on `bg-on-primary`, and that pair
 *    is white in *both* themes.** A QR is read by a camera, not by a person, so it
 *    cannot follow the app's dark theme: light modules on a white ground do not
 *    scan. `on-primary` is the one token that does not change between the two
 *    themes, which is why this is the single surface in the app that does not
 *    invert — and the pair is declared in the contrast table (`tokens.ts`) rather
 *    than assumed, so the gate measures it.
 *  * **The quiet zone is four modules**, which the standard requires, and it is
 *    part of the SVG's coordinate space rather than CSS padding, so the white
 *    ground is drawn by the SVG's own background and cannot be clipped off by a
 *    parent's `overflow`.
 *
 * The whole code is one `<path>` rather than a rect per module: a version-10 QR is
 * a few thousand modules, and a few thousand DOM nodes is a screen that stutters
 * while its countdown ticks.
 *
 * The frame's provider wordmarks (`Gopay`, `OVO`, `ShopeePay`, `BCA`) are **not
 * rendered**: they are brand names the design drew, and nothing in the payload
 * says which e-wallet the student will scan with. A list of names this app put
 * there would be a claim about a payment the gateway has not seen.
 */
function QrCode({ payload, label }: { payload: string; label: string }) {
  const modules = qrModules(payload)
  if (modules === null) {
    // An encoder failure is not an exception here: the screen still has the
    // amount, the state and the order reference, and a thrown error inside a poll
    // would take the whole screen down. The code is simply absent.
    return null
  }

  const { count, path } = modules
  // Four modules of margin on each side, in the same units as the modules, so the
  // quiet zone scales with the code.
  const margin = 4
  const size = count + margin * 2

  return (
    <div className="flex flex-col items-center gap-2">
      <div className="w-[min(100%,18rem)] rounded-card bg-on-primary p-2">
        <svg
          viewBox={`${-margin} ${-margin} ${size} ${size}`}
          role="img"
          aria-label={label}
          className="block h-auto w-full text-primary"
          // The QR's own scale: crisp edges, no antialiasing between modules,
          // which is what keeps a downscaled code readable.
          shapeRendering="crispEdges"
        >
          <path d={path} fill="currentColor" />
        </svg>
      </div>
      <p className="max-w-prose text-center text-small text-muted">{label}</p>
    </div>
  )
}
