import { useMutation, useQuery } from '@tanstack/react-query'

import { api } from '../client'

/**
 * `GET /plans`, `POST /billing/checkout` and the payment poll.
 *
 * Three rules from the server shape this module, and each is visible in its types.
 *
 * **The amount is the server's.** `PaymentView.amount_idr` is what
 * `billing::price` computed from `config/plans.toml`, the student price and any
 * discount code, and it is the number the webhook later checks the payment
 * against (A294). Nothing here adds, subtracts or rounds: a client-side total that
 * disagrees with the charge is a support ticket at best, and `checkout.ts`'s
 * `orderLines` reads the row's own itemisation rather than re-deriving it.
 *
 * **Prices may be absent.** `GET /plans` omits `price_idr_*` and
 * `credit_packs[].price_idr` where `show_prices` is false (A305), which is what a
 * phone gets: the plan names, the limits and the features and no prices. So every
 * price field is optional here rather than `number`, and a screen renders no
 * price rather than a `0` standing in for "not shown". `show_prices` and
 * `purchase_allowed` are the server's own reading of `billing_policy.toml`, and
 * they are a second opinion on what `features/billing/policy.ts` decides — the
 * client policy is what keeps the *phone build* from containing a purchase
 * surface, and the server's flags are what keep a patched client from being told
 * one.
 *
 * **The plan screen does not call this module for the plan, the credits or the
 * renewal date.** Those are `GET /usage` (Phase 7), which is the route the spec's
 * own plan screen reads and the one the entitlement decisions are built from. A
 * second route returning the same facts would be a second source of truth, and the
 * two would disagree the day one of them changed.
 */

/** One `[credit_packs]` entry as `GET /plans` sends it. */
export interface PlanCreditPack {
  /** The pack's identity on the wire: a checkout names the pack by this. */
  credits: number
  /** Absent where the server's policy hides prices. */
  price_idr?: number
}

/**
 * One plan's limits, exactly as `entitlements::limits_view` builds them.
 *
 * The values are the config's own, not a normalised shape: an uncapped axis is the
 * string `"unlimited"` and a genuine zero is `0` — which is why this cannot be
 * `Record<string, number | null>`. `config/plans.toml` says it in as many words
 * (free allows **zero** Google accounts while power allows unlimited, and a `0`
 * sentinel for "unlimited" would read free as uncapped). A screen that collapsed
 * the two would tell a student they may connect a Google account they may not.
 */
export type PlanLimits = Record<string, number | string | boolean | string[]>

/** One `[plans.<id>]` block as `GET /plans` sends it. */
export interface PlanOffer {
  /** The plan id the config uses, e.g. `"pro"`. */
  id: string
  /** The display name, e.g. `"Pro"`. */
  name: string
  /** Every limit axis, as the matrix declares it. */
  limits: PlanLimits
  /** The discount the matrix grants a verified student, in percent. */
  student_percent_off: number
  byok: boolean
  credits_per_month: number | null
  price_idr_monthly?: number
  price_idr_yearly?: number
  /** The student price, computed by the same `billing::price` the checkout charges from. */
  student_price_idr_monthly?: number
  student_price_idr_yearly?: number
}

/** The body of `GET /plans`. */
export interface PlansView {
  plans: PlanOffer[]
  credit_packs: PlanCreditPack[]
  /** False on a platform whose policy hides prices (a phone). */
  show_prices: boolean
  /** False where `POST /billing/checkout` would be refused with 403 `feature_locked`. */
  purchase_allowed: boolean
  /** The currency the matrix prices in, e.g. `"IDR"`. */
  currency: string
  /** The plan in effect right now, which is `GET /usage`'s `plan` too. */
  current_plan: string
  student_verified: boolean
}

/**
 * One payment row: what `POST /billing/checkout` returns, and what
 * `GET /billing/payments/:id` polls.
 *
 * The same shape from both routes by construction — the server builds them from
 * one `select` — so the checkout screen's first render and its tenth poll cannot
 * be reading different fields.
 */
export interface PaymentView {
  id: string
  /** The order id the gateway knows this by. */
  order_id: string
  /** `subscription` or `credit_pack`. */
  kind: string
  plan_id: string | null
  /** `monthly` or `yearly`; null for a credit pack. */
  billing_period: string | null
  /** The credits a pack grants; null for a subscription. */
  credits: number | null
  /** The amount to pay, computed by `billing::price`. Never re-derived on the client. */
  amount_idr: number
  /** The list price before the discount, so the screen can itemise the charge. */
  amount_before_discount_idr: number
  discount_percent: number
  discount_code: string | null
  student_discount: boolean
  /** The QRIS payload to render. Null only between the row and the gateway's answer. */
  qr_string: string | null
  /** An absolute instant (A6). The countdown is this, against the client's clock. */
  expires_at: string
  /** `pending` | `paid` | `expired` | `failed` | `refunded` | `review`. */
  status: string
  paid_at: string | null
}

/** The body of `POST /billing/checkout`. */
export interface CheckoutBody {
  plan_id?: string
  billing_period?: 'monthly' | 'yearly'
  /** A credit pack, named by the credits it grants. */
  credit_pack?: number
  discount_code?: string
}

export const billingKeys = {
  plans: ['plans'] as const,
  payment: (id: string) => ['billing', 'payments', id] as const,
  invoices: ['billing', 'invoices'] as const,
}

/**
 * The plan matrix, as the server reads it.
 *
 * The platform is the device row (A144), so this answers differently on a phone
 * than on the web — no prices, no packs' prices — without the client having to ask
 * for a filtered view.
 */
export function usePlans() {
  return useQuery({
    queryKey: billingKeys.plans,
    queryFn: async ({ signal }) => api.get<PlansView>('/plans', { signal }),
  })
}

/**
 * One payment, polled while it is still open.
 *
 * `refetchInterval` is a function of the data rather than a number, which is what
 * stops the poll on a terminal status: a paid invoice refetched forever is a
 * request every three seconds from every student who has finished paying, and the
 * screen's own state is derived from the row rather than from the poll.
 *
 * **A row the server has not answered yet does keep polling**, and that is the
 * lost-webhook path rather than an oversight: the state a student sits in when a
 * callback went missing is `pending` with a live expiry, and the polling fallback
 * (A301) is the only thing that moves it. So the interval is keyed on the row
 * being *open*, not on the client's countdown.
 */
export function usePayment(id: string | undefined) {
  return useQuery({
    queryKey: billingKeys.payment(id ?? ''),
    enabled: id !== undefined,
    queryFn: async ({ signal }) => api.get<PaymentView>(`/billing/payments/${id ?? ''}`, { signal }),
    refetchInterval: (query) => {
      const status = query.state.data?.status
      if (status === undefined) return 3000
      return status === 'pending' ? 3000 : false
    },
  })
}

/**
 * `POST /billing/checkout` — mint (or reuse) the invoice for a plan or a pack.
 *
 * A mutation rather than a query, because it writes a `payments` row and calls the
 * gateway. It is deliberately *not* retried automatically: the server's reuse rule
 * (A307) makes a second call for the same plan and period return the pending
 * invoice, so a retry is safe — but a retry the user did not ask for is a second
 * gateway round trip for a code they may already be looking at.
 */
export function useCheckout() {
  return useMutation({
    mutationFn: async (body: CheckoutBody) =>
      api.post<PaymentView>('/billing/checkout', body),
    retry: false,
  })
}

/**
 * `GET /billing/invoices` — the account's own payment history, newest first.
 *
 * An object rather than a bare array, so a cursor or a total can be added later
 * without breaking a client that already parses it. The elements are the same
 * `PaymentView` the checkout screen polls, so the history and the live invoice
 * render from one shape.
 *
 * **This route does not re-check expiry**, deliberately: it is a read of what
 * happened, and a list that rewrote its own rows while a student was reading their
 * history would be a surprise. The live invoice is the one the checkout screen
 * polls by id, and that one does re-check.
 *
 * The bound is the server's (50 rows). Nothing here pages: a panel that showed a
 * paging control for a list this short would be a control that never does
 * anything.
 */
export function useInvoices() {
  return useQuery({
    queryKey: billingKeys.invoices,
    queryFn: async ({ signal }) =>
      api.get<{ invoices: PaymentView[] }>('/billing/invoices', { signal }),
  })
}
