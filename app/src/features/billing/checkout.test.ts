import { describe, expect, it } from 'vitest'

import { canStartOver, checkoutState, countdown, formatAmount, orderLines } from './checkout'
import type { PaymentView } from '@/api/queries/billing'

/**
 * The checkout screen's arithmetic, tested without a screen.
 *
 * Everything here takes its clock as a parameter, which is the whole reason the
 * module exists: a countdown that reads `Date.now()` inside cannot be tested at
 * the boundary (the second the invoice expires), and the boundary is the only
 * interesting moment it has. The same discipline `features/planner/localSchedule`
 * and the capture draft's day follow.
 *
 * The two properties a support ticket comes from are pinned first: the countdown
 * is the **server's** expiry measured against the client's clock (never a duration
 * the client invented), and the total is the **server's** `amount_idr` (never a
 * subtraction the client did).
 */

const NOW = Date.parse('2026-10-12T09:00:00Z')
const EXPIRES = '2026-10-12T09:15:00Z'

/** One payment row as `POST /billing/checkout` and the poll both answer it. */
function payment(overrides: Partial<PaymentView> = {}): PaymentView {
  return {
    id: '3f1c1f4e-0000-4000-8000-000000000001',
    order_id: 'stvdy_0f1e2d3c',
    kind: 'subscription',
    plan_id: 'pro',
    billing_period: 'monthly',
    credits: null,
    amount_idr: 5000,
    amount_before_discount_idr: 10000,
    discount_percent: 50,
    discount_code: null,
    student_discount: true,
    qr_string: 'MOCKQRIS:stvdy_0f1e2d3c:5000',
    expires_at: EXPIRES,
    status: 'pending',
    paid_at: null,
    ...overrides,
  }
}

describe('the countdown', () => {
  it('the countdown is derived from the server expiry, not the local clock', () => {
    // A client an hour fast still shows the server's remaining time, because the
    // only clock in the subtraction is the injected one.
    expect(countdown(EXPIRES, Date.parse('2026-10-12T08:15:30Z'))).toEqual({
      mm: 59,
      ss: 30,
      done: false,
    })
    expect(countdown(EXPIRES, Date.parse('2026-10-12T10:00:00Z'))).toEqual({
      mm: 0,
      ss: 0,
      done: true,
    })
  })

  it('is done at the expiry instant and never goes negative', () => {
    const expiry = Date.parse(EXPIRES)
    expect(countdown(EXPIRES, expiry - 1000)).toEqual({ mm: 0, ss: 1, done: false })
    // The boundary itself: `now >= expiresAt` is done, not one second left.
    expect(countdown(EXPIRES, expiry)).toEqual({ mm: 0, ss: 0, done: true })
    expect(countdown(EXPIRES, expiry + 3_600_000)).toEqual({ mm: 0, ss: 0, done: true })
  })

  it('treats an expiry it cannot read as past', () => {
    // Not "0:00 left, still waiting": an invoice whose expiry the server sent in a
    // shape this build cannot parse is one no student should be shown a code for.
    expect(countdown('not a timestamp', NOW)).toEqual({ mm: 0, ss: 0, done: true })
    expect(checkoutState(payment({ expires_at: 'not a timestamp' }), NOW)).toBe('expired')
  })
})

describe('the state machine', () => {
  it('a paid payment never renders a new QR', () => {
    // Every expiry, including one in the past: paid outranks the clock, because a
    // student whose invoice expired while our webhook was in flight has paid.
    for (const expires of [EXPIRES, '2026-10-12T08:00:00Z', '2030-01-01T00:00:00Z']) {
      expect(checkoutState(payment({ status: 'paid', expires_at: expires }), NOW)).toBe('paid')
    }
  })

  it('an expired invoice offers a new QR and a review does not', () => {
    const expired = checkoutState(payment({ status: 'expired' }), NOW)
    const byClock = checkoutState(payment(), Date.parse('2026-10-12T09:15:00Z'))
    const review = checkoutState(payment({ status: 'review' }), NOW)

    expect(expired).toBe('expired')
    // The server re-checks expiry on every poll, and the client's own clock
    // reaching it is the same state rather than a second one.
    expect(byClock).toBe('expired')
    expect(review).toBe('review')

    expect(canStartOver(expired)).toBe(true)
    expect(canStartOver(byClock)).toBe(true)
    expect(canStartOver(review)).toBe(false)
  })

  it("reads the server's own statuses without inventing a sixth meaning", () => {
    expect(checkoutState(payment(), NOW)).toBe('waiting')
    expect(checkoutState(payment({ status: 'failed' }), NOW)).toBe('failed')
    expect(checkoutState(payment({ status: 'refunded' }), NOW)).toBe('failed')
    // A status this build does not know is not success and not failure: the row is
    // shown as under review, which is the only claim that is true of it.
    expect(checkoutState(payment({ status: 'quantum' }), NOW)).toBe('review')
  })

  it('separates "the code is dead" from "the server still holds the payment open"', () => {
    // The clock can end the *code* — the countdown, the expired state — without
    // ending the *poll*: the poll continues while the server says `pending`,
    // because a webhook that is merely late must still be able to move the screen
    // to paid. Only the server's own status is terminal, which is why nothing here
    // decides when to stop asking.
    const stale = payment()
    expect(checkoutState(stale, Date.parse('2026-10-12T09:20:00Z'))).toBe('expired')
    expect(stale.status).toBe('pending')
  })
})

describe('the order summary', () => {
  it("renders the server's total and never a total it computed", () => {
    // The row says 4,900 IDR while the two itemised numbers say 10,000 less 50%.
    // A screen that re-derived the arithmetic would print 5,000 here, and that
    // disagreement with the charge is the defect this pins.
    const lines = orderLines(
      payment({ amount_idr: 4900, amount_before_discount_idr: 10000, discount_percent: 50 }),
      'en',
    )
    const total = lines.at(-1)
    expect(total?.labelKey).toBe('total')
    expect(total?.value).toBe(formatAmount(4900, 'en'))
    expect(total?.value).not.toBe(formatAmount(5000, 'en'))
  })

  it('names a discount once, and says which one it is', () => {
    expect(orderLines(payment(), 'en').map((line) => line.labelKey)).toEqual([
      'beforeDiscount',
      'discountStudent',
      'total',
    ])

    const coded = orderLines(
      payment({ student_discount: false, discount_code: 'UTS2026', discount_percent: 20 }),
      'en',
    )
    expect(coded.map((line) => line.labelKey)).toEqual(['beforeDiscount', 'discountCode', 'total'])
    expect(coded[1]?.value).toBe('20%')
  })

  it('shows no discount line when the charge has none', () => {
    const lines = orderLines(
      payment({
        amount_idr: 10000,
        amount_before_discount_idr: 10000,
        discount_percent: 0,
        student_discount: false,
      }),
      'en',
    )
    expect(lines.map((line) => line.labelKey)).toEqual(['total'])
  })

  it('names the credits a pack grants, and only for a pack', () => {
    const pack = orderLines(
      payment({
        kind: 'credit_pack',
        plan_id: null,
        billing_period: null,
        credits: 500,
        amount_idr: 12500,
        amount_before_discount_idr: 12500,
        discount_percent: 0,
        student_discount: false,
      }),
      'en',
    )
    expect(pack.map((line) => line.labelKey)).toEqual(['credits', 'total'])
    // A subscription carries no credit amount, so no line claims one rather than
    // a `0` standing in for "not applicable".
    expect(orderLines(payment(), 'en').some((line) => line.labelKey === 'credits')).toBe(false)
  })

  it("formats the amount in the file's currency, in the reader's language", () => {
    // `config/plans.toml [meta] currency` is the one price value the app reads at
    // build time, because a pure function handed a payment row has no payload to
    // read a currency from. The three differences that matter are all here:
    // Indonesian writes `Rp` where English has no rupiah symbol and names the code
    // `IDR`, Indonesian groups with a full stop (`10.000`) where English groups
    // with a comma (`10,000`), and the separator between the symbol and the digits
    // is a **non-breaking space** in both — which is what stops `Rp` and `10.000`
    // wrapping onto two lines in a narrow card, on the money copy. Neither string
    // is a literal in the module.
    expect(formatAmount(10000, 'id')).toBe('Rp\u00a010.000')
    expect(formatAmount(10000, 'en')).toBe('IDR\u00a010,000')
    // The ledger's own formatter is reused for a count, so `1.500` means the same
    // thing in a pack's credit line as it does in the credit meter.
    expect(orderLines(payment({ kind: 'credit_pack', credits: 1500 }), 'id')[0]?.value).toBe(
      '1.500',
    )
  })
})
