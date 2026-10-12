import type { PaymentView } from '@/api/queries/billing'
import { formatCount } from '@/features/billing/CreditMeter'
import { PLAN_CURRENCY } from '@/features/billing/plans'
import { INTL_LOCALE, isLanguage } from '@/i18n'

/**
 * The checkout screen's arithmetic, and none of its rendering.
 *
 * **Nothing here reads a clock.** Every function that needs the time takes `now`
 * as a parameter, which is what makes the expiry boundary testable at all — the
 * second an invoice dies is the only interesting moment a countdown has, and a
 * `Date.now()` inside would make it unreachable from a test. It is the same
 * discipline the planner and `features/planner/localSchedule` follow.
 *
 * **Nothing here computes a total.** `orderLines` reads `amount_idr`,
 * `amount_before_discount_idr`, `discount_percent` and `discount_code` off the
 * payment row the server sent, and the itemisation is the server's own. `A298`
 * makes the student promo and a discount code one decision (the larger percentage
 * wins, they never stack), made once by `billing::price`; a client that re-derived
 * the arithmetic would be a second implementation of that rule, and a client-side
 * total that disagrees with the charge is a support ticket at best.
 *
 * This is a `.ts` module rather than part of the screen because the screen cannot
 * be asked these questions: "is the poll still running", "does this state offer a
 * new code" and "what does the summary say" are decisions, and a decision buried
 * in JSX is one a test can only reach by rendering and pattern-matching text.
 */

/**
 * The six states the checkout screen can be in.
 *
 * Five are the spec's — `waiting`, `paid`, `expired`, a `review` (a mismatched
 * amount, A294) and `failed` — and the sixth is this client's own: `offline`, the
 * request that did not reach the server.
 *
 * `waiting` is the one that carries the weight. It is the state a student sits in
 * when the webhook is lost, when the gateway is slow, and when the money has
 * genuinely not arrived — three different situations with one honest sentence
 * between them, and the polling fallback (A301) is what moves it. It must not read
 * as failure (the student has a live code and may still pay it) or as success (no
 * payment has been confirmed), and neither of those may be said in colour alone.
 */
export type CheckoutState = 'waiting' | 'paid' | 'expired' | 'review' | 'failed' | 'offline'

/** The states a poll can stop on, because the server has made its decision. */
const TERMINAL: CheckoutState[] = ['paid', 'expired', 'review', 'failed']

/** Whether a state is an answer rather than a moment. */
export function isTerminal(state: CheckoutState): boolean {
  return TERMINAL.includes(state)
}

/**
 * Whether the screen offers "start over" — a fresh invoice, or the route back.
 *
 * Only the two states where starting over is the remedy: an invoice that ran out
 * of time, and the account that never reached the server. A `review` is waiting on
 * a human (A294), so a retry there would be a second charge for a payment that is
 * already being looked at; `paid` needs nothing; `failed` is the gateway's refusal
 * and its own path back through the pricing page.
 */
export function canStartOver(state: CheckoutState): boolean {
  return state === 'expired' || state === 'offline'
}

/**
 * The state, from the server's row and the client's clock.
 *
 * The server's status decides first, in every case: `paid` outranks a past expiry
 * (a webhook that arrived after the code died is still a payment), and `review`
 * outranks the clock (a mismatch is a fact about the money, not about time). Only
 * a row the server still holds open is decided by the clock, and then only in one
 * direction — `now >= expires_at` means the code is dead.
 *
 * A status this build does not recognise reads as `review`, not as `failed` and
 * certainly not as `paid`: "under review" is the only claim that is true of a row
 * whose meaning we do not know, and it is the one that sends the student to a human
 * rather than to a second payment.
 */
export function checkoutState(view: PaymentView, now: number): CheckoutState {
  switch (view.status) {
    case 'paid':
      return 'paid'
    case 'review':
      return 'review'
    case 'failed':
    case 'refunded':
      return 'failed'
    case 'expired':
      return 'expired'
    case 'pending':
      return countdown(view.expires_at, now).done ? 'expired' : 'waiting'
    default:
      return 'review'
  }
}

/**
 * `mm:ss` to the server's expiry, clamped at zero.
 *
 * The expiry is the server's absolute instant (A6) and `now` is the client's
 * clock, so a client whose clock is wrong shows the *server's* remaining time
 * measured wrongly rather than a duration it invented. An expiry that cannot be
 * parsed is treated as past: an invoice whose deadline we cannot read is one no
 * student should be shown a code for, and `done` is the honest answer.
 *
 * Never negative, because a negative countdown is a screen telling a student they
 * have minus four minutes to pay.
 */
export function countdown(expiresAt: string, now: number): { mm: number; ss: number; done: boolean } {
  const expiry = Date.parse(expiresAt)
  if (Number.isNaN(expiry)) return { mm: 0, ss: 0, done: true }
  const remaining = Math.max(0, expiry - now)
  const done = remaining === 0
  const total = Math.floor(remaining / 1000)
  return { mm: Math.floor(total / 60), ss: total % 60, done }
}

/**
 * The countdown as `mm:ss`.
 *
 * Padded by hand rather than through `Intl`, and that is the point: the clock is a
 * fixed-width field the student watches count down, and `Intl`'s duration support
 * differs between engines and between the two locales this app ships. The screen
 * interpolates this into its own translated sentence, so the format has to be
 * identical in English and Indonesian — which means it must not be a locale's
 * decision. Exported because the screen renders it and `checkout.test.ts` asserts
 * the same string the screen shows.
 */
export function clock(mm: number, ss: number): string {
  return `${String(mm).padStart(2, '0')}:${String(ss).padStart(2, '0')}`
}

/**
 * An amount of the matrix's currency, in the reader's language.
 *
 * `Intl.NumberFormat` rather than a template string, because the two locales
 * genuinely differ: `id-ID` writes `Rp 10.000` with a non-breaking space and a full
 * stop for the thousands, `en-GB` has no rupiah symbol and writes `IDR 10,000`.
 * Hand-formatting one of those would be wrong in the other, and this is the money
 * copy in a product sold in Indonesia.
 *
 * The currency is `config/plans.toml`'s `[meta] currency` — the same value the
 * server prices in — because this function is pure and a payment row carries an
 * amount with no currency beside it. When the file cannot be read there is no
 * symbol, which is a number a student can still check against the charge rather
 * than a plausible-looking one this app invented.
 */
export function formatAmount(amountIdr: number, language: string): string {
  const tag = isLanguage(language) ? INTL_LOCALE[language] : INTL_LOCALE.en
  // No currency when the file could not be read: a symbol this app invented is a
  // worse answer than a bare number the student can check against the charge.
  const options: Intl.NumberFormatOptions =
    PLAN_CURRENCY === null
      ? { maximumFractionDigits: 0 }
      : { style: 'currency', currency: PLAN_CURRENCY, maximumFractionDigits: 0 }
  return new Intl.NumberFormat(tag, options).format(amountIdr)
}

/**
 * One line of the order summary.
 *
 * `labelKey` is a key rather than a sentence and `params` carries the values it
 * interpolates, because the caller is the only place with a language: a string
 * built here would be English shown to an Indonesian user, which is the defect the
 * i18n contract test exists to catch and the one place it structurally cannot see
 * (copy in a `.ts` table).
 */
export interface OrderLine {
  labelKey: string
  params?: Record<string, string>
  value: string
}

/**
 * The order summary, from the server's own breakdown.
 *
 * The list price is shown only when it differs from what is charged, so a plan
 * with no discount is one line rather than a strike-through that says "10,000 was
 * 10,000". The discount line is named for what actually applied: `student_discount`
 * and a `discount_code` are mutually exclusive by A298 (the larger percentage wins,
 * they do not stack), so at most one of the two lines can appear, and a row that
 * somehow carries both is itemised as the code — the student promo is the one the
 * screen can explain from the account.
 *
 * The total is `amount_idr` **verbatim**. That is the number the webhook checks the
 * gateway's claim against (A294) and the number the student will see on their
 * statement.
 */
export function orderLines(view: PaymentView, language: string): OrderLine[] {
  const lines: OrderLine[] = []

  // A pack's identity is the credits it grants, so that is what the line names —
  // not a "Pro, monthly" a pack does not have.
  if (view.kind === 'credit_pack' && view.credits !== null) {
    lines.push({
      labelKey: 'credits',
      value: formatCount(view.credits, language),
    })
  }

  if (view.amount_before_discount_idr > view.amount_idr) {
    lines.push({
      labelKey: 'beforeDiscount',
      value: formatAmount(view.amount_before_discount_idr, language),
    })
    if (view.discount_code !== null) {
      lines.push({
        labelKey: 'discountCode',
        params: { code: view.discount_code },
        value: `${view.discount_percent}%`,
      })
    } else {
      lines.push({
        labelKey: 'discountStudent',
        value: `${view.discount_percent}%`,
      })
    }
  }

  lines.push({ labelKey: 'total', value: formatAmount(view.amount_idr, language) })
  return lines
}
