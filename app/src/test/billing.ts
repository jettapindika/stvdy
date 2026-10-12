import type { PaymentView, PlansView } from '@/api/queries/billing'
import type { Usage } from '@/api/queries/usage'

import { jsonResponse, mockFetch } from './harness'
import { USAGE, USER } from './usage'

/**
 * The billing screens' fixtures: `GET /plans`, the checkout routes and the poll.
 *
 * `@/test/usage` holds the `GET /usage` payload both plan screens read. This file
 * holds what the *purchase* half of `/billing` reads, and it serves `/usage` too,
 * because `mockFetch` installs one handler for the whole test: a second call would
 * replace the first and the screen would 404 on whichever route it asked for
 * second.
 *
 * The prices here are **deliberately not the values in `config/plans.toml`**. That
 * is the whole design of the honesty assertion in `BillingScreen.test.tsx`: a
 * screen that rendered the file's numbers instead of the payload's would agree
 * with `config/plans.toml` and disagree with the server, and the test would pass.
 * So the fixture's `pro` monthly price is `12345` where the file says `10000`, and
 * the pack prices differ the same way. A screen that prints a rupiah amount this
 * handler did not send fails.
 *
 * The `/plans` body is shaped exactly as `crates/server/src/billing/routes.rs::
 * plans` builds it, including the two facts a phone gets: `show_prices: false`
 * with the price fields **absent** rather than zero, and `purchase_allowed: false`.
 */

/** The price the fixture's `pro` plan charges monthly. Not the file's `10000`. */
export const PRO_MONTHLY = 12345

/** The price the fixture's `power` plan charges monthly. Not the file's `25000`. */
export const POWER_MONTHLY = 24680

/** The fixture's first credit pack: its price differs from the file's `3000`. */
export const PACK_CREDITS = 100
export const PACK_PRICE = 4321

export const PLANS: PlansView = {
  plans: [
    {
      id: 'free',
      name: 'Free',
      limits: {
        plan: 'free',
        name: 'Free',
        workspaces: 2,
        sources_total: 20,
        max_file_mb: 20,
        storage_mb: 200,
        devices_max: 2,
        credits_per_month: 60,
        reports_per_month: 1,
        flashcards_per_month: 30,
        quizzes_per_month: 2,
        google_accounts_max: 0,
        export_formats: ['md'],
      },
      student_percent_off: 0,
      byok: false,
      credits_per_month: 60,
      price_idr_monthly: 0,
      price_idr_yearly: 0,
      student_price_idr_monthly: 0,
      student_price_idr_yearly: 0,
    },
    {
      id: 'pro',
      name: 'Pro',
      limits: {
        plan: 'pro',
        name: 'Pro',
        workspaces: 'unlimited',
        sources_total: 150,
        max_file_mb: 50,
        storage_mb: 2000,
        devices_max: 5,
        credits_per_month: 500,
        reports_per_month: 'unlimited',
        flashcards_per_month: 'unlimited',
        quizzes_per_month: 'unlimited',
        google_accounts_max: 1,
        export_formats: ['md', 'pdf', 'docx'],
      },
      student_percent_off: 50,
      byok: false,
      credits_per_month: 500,
      price_idr_monthly: PRO_MONTHLY,
      price_idr_yearly: 98765,
      student_price_idr_monthly: 6172,
      student_price_idr_yearly: 49382,
    },
    {
      id: 'power',
      name: 'Power',
      limits: {
        plan: 'power',
        name: 'Power',
        workspaces: 'unlimited',
        sources_total: 500,
        max_file_mb: 100,
        storage_mb: 10000,
        devices_max: 10,
        credits_per_month: 2000,
        reports_per_month: 'unlimited',
        flashcards_per_month: 'unlimited',
        quizzes_per_month: 'unlimited',
        google_accounts_max: 'unlimited',
        export_formats: ['md', 'pdf', 'docx', 'csv', 'json'],
      },
      student_percent_off: 50,
      byok: true,
      credits_per_month: 2000,
      price_idr_monthly: POWER_MONTHLY,
      price_idr_yearly: 198765,
      student_price_idr_monthly: 12340,
      student_price_idr_yearly: 99382,
    },
  ],
  credit_packs: [
    { credits: PACK_CREDITS, price_idr: PACK_PRICE },
    { credits: 500, price_idr: 9876 },
  ],
  show_prices: true,
  purchase_allowed: true,
  currency: 'IDR',
  current_plan: 'free',
  student_verified: false,
}

/**
 * The same matrix as a phone sees it: no prices anywhere, and no checkout.
 *
 * This is what `GET /plans` answers when `show_prices` is false for the device's
 * platform (A305), and it is the shape the mobile assertions run against — a
 * screen that fell back to a default price would have nothing to fall back to,
 * which is the point.
 */
export const PLANS_WITHOUT_PRICES: PlansView = {
  ...PLANS,
  plans: PLANS.plans.map((plan) => ({
    id: plan.id,
    name: plan.name,
    limits: plan.limits,
    student_percent_off: plan.student_percent_off,
    byok: plan.byok,
    credits_per_month: plan.credits_per_month,
  })),
  credit_packs: PLANS.credit_packs.map((pack) => ({ credits: pack.credits })),
  show_prices: false,
  purchase_allowed: false,
}

/** One pending subscription payment, as the checkout route and the poll send it. */
export const PAYMENT: PaymentView = {
  id: '3f1c1f4e-0000-4000-8000-000000000001',
  order_id: 'stvdy_0f1e2d3c',
  kind: 'subscription',
  plan_id: 'pro',
  billing_period: 'monthly',
  credits: null,
  amount_idr: 6172,
  amount_before_discount_idr: PRO_MONTHLY,
  discount_percent: 50,
  discount_code: null,
  student_discount: true,
  qr_string: 'MOCKQRIS:stvdy_0f1e2d3c:6172',
  // Fixed, not computed: the countdown tests inject `now`, and a fixture that read
  // the clock would make every one of them a race.
  expires_at: '2026-10-12T09:15:00Z',
  status: 'pending',
  paid_at: null,
}

/** What a screen test hands `serveBilling` to script one route's answer. */
export interface BillingScript {
  usage?: Usage
  plans?: PlansView
  payment?: PaymentView
  /** The answer `POST /billing/checkout` gives, or a status to fail with. */
  checkout?: PaymentView | number
  /** The body `GET /billing/invoices` answers with. */
  invoices?: PaymentView[]
}

/**
 * Serves every route the billing screens read, from one handler.
 *
 * A route the script does not name answers with a payload rather than a 404, so a
 * test that forgets one still renders the screen it is about instead of an error
 * notice — except `invoices`, which is deliberately **absent** unless asked for,
 * because the panel's "no history" state is a state worth testing and a fixture
 * that always answered would make it unreachable.
 */
export function serveBilling(script: BillingScript = {}) {
  return mockFetch((url, init) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, USER)
    if (path === '/usage') return jsonResponse(200, script.usage ?? USAGE)
    if (path === '/plans') return jsonResponse(200, script.plans ?? PLANS)
    if (path === '/billing/checkout') {
      const answer = script.checkout ?? PAYMENT
      return typeof answer === 'number'
        ? jsonResponse(answer, {
            code: 'validation',
            message: 'no',
            min_plan: null,
            limit: null,
            used: null,
          })
        : jsonResponse(200, answer)
    }
    if (path.startsWith('/billing/payments/')) return jsonResponse(200, script.payment ?? PAYMENT)
    if (path === '/billing/invoices') return jsonResponse(200, { invoices: script.invoices ?? [] })
    return jsonResponse(404, {
      code: 'not_found',
      message: `no fixture for ${path} ${init?.method ?? 'GET'}`,
      min_plan: null,
      limit: null,
      used: null,
    })
  })
}
