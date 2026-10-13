import { screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { AdminScreen } from './AdminScreen'
import { setAccessToken } from '@/lib/tokens'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'

/**
 * The admin console.
 *
 * The cases here are the ones a plausible implementation gets wrong, and each is a
 * claim the screen makes about a number rather than a layout:
 *
 * 1. **A tripwire with no data is not "not triggered".** The server sends
 *    `Option<bool>` precisely because `null` ("the range cannot answer this") and
 *    `false` ("it is not happening") are different facts, and a renderer that
 *    collapsed them would report safety it never measured. `plan_loss_two_months_running`
 *    is `null` whenever the range holds fewer than two months, which is the ordinary
 *    case on a young deployment — so this is the *common* path, not an edge.
 * 2. **A percentage with no denominator is not zero.** A month with no revenue has
 *    no contribution margin, and `0%` reads as a measured break-even.
 * 3. **"Never active" is a state, not a blank.** An account with no `usage_events`
 *    row has `last_active_at: null`, and rendering that as an empty cell makes it
 *    indistinguishable from a rendering failure.
 *
 * The console is web-only by design, so the phone case asserts the refusal rather
 * than a cramped table.
 */

const ADMIN_USER = {
  id: 'u1',
  email: 'operator@example.test',
  email_verified: true,
  email_verified_at: '2026-10-11T08:00:00Z',
  plan_id: 'free',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  is_admin: true,
  created_at: '2026-10-11T08:00:00Z',
}

const USERS = {
  from: '2026-09-12T00:00:00Z',
  to: '2026-10-12T00:00:00Z',
  users: [
    {
      id: 'u1',
      email: 'active@example.test',
      plan_id: 'pro',
      created_at: '2026-09-01T08:00:00Z',
      requests: 42,
      sources_added: 3,
      last_active_at: '2026-10-12T09:30:00Z',
      is_admin: true,
    },
    {
      id: 'u2',
      email: 'quiet@example.test',
      plan_id: 'free',
      created_at: '2026-10-01T08:00:00Z',
      requests: 0,
      sources_added: 0,
      last_active_at: null,
      is_admin: false,
    },
  ],
}

const PAYMENTS = {
  count: 1,
  payments: [
    {
      id: 'p1',
      user_id: 'u1',
      kind: 'subscription',
      plan_id: 'pro',
      billing_period: 'monthly',
      credits: null,
      provider: 'qris',
      order_id: 'ORD-2026-0001',
      amount_idr: 10000,
      gateway_fee_idr: 700,
      status: 'review',
      review_reason: 'amount mismatch',
      expires_at: '2026-10-12T05:50:00Z',
      paid_at: null,
      created_at: '2026-10-12T05:35:00Z',
    },
  ],
}

/** The month the console's summary cards read: real revenue, real cost, real zero fee. */
const MARGIN = {
  from: '2026-09-12T00:00:00Z',
  to: '2026-10-12T00:00:00Z',
  idr_per_usd: 16000,
  months: [
    {
      month: '2026-10',
      per_plan: [
        {
          plan_id: 'pro',
          provider_cost_usd: 0.5625,
          byok_cost_usd: 0,
          revenue_idr: 45000,
          gross_margin_idr: 36000,
          ceiling_usd: 10,
        },
      ],
      revenue_idr: 45000,
      gateway_fee_idr: 3150,
      free_utilization: 0.28,
      paying: 3,
      free_accounts: 4,
      conversion_of_free: 0.75,
      conversion_of_total: 0.43,
      as_of: '2026-10-12T00:00:00Z',
    },
  ],
  // `plan_loss_two_months_running` is `null` on a range with fewer than two months,
  // and `free_spend_over_quarter_of_revenue` is `true` here — so one row of each
  // kind is rendered and the two must not read the same.
  tripwires: {
    free_utilization_over_30: false,
    conversion_under_3: false,
    free_spend_over_quarter_of_revenue: true,
    plan_loss_two_months_running: null,
  },
}

function setup(overrides: { users?: unknown; margin?: unknown } = {}) {
  installFakePlatform('macos')
  mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/auth/me') return jsonResponse(200, ADMIN_USER)
    if (path === '/admin/users') return jsonResponse(200, overrides.users ?? USERS)
    if (path === '/admin/payments') return jsonResponse(200, PAYMENTS)
    if (path === '/admin/margin') return jsonResponse(200, overrides.margin ?? MARGIN)
    if (path === '/usage') return jsonResponse(200, {})
    return jsonResponse(404, { code: 'not_found', message: 'no' })
  })
}

beforeEach(() => {
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the admin console', () => {
  it('renders a tripwire with no data as not measured, not as clear', async () => {
    setup()
    renderWithProviders(<AdminScreen />, { route: '/admin' })

    const label = await screen.findByText('Two consecutive months at a loss')
    const row = label.closest('li')
    expect(row).not.toBeNull()
    // The distinction the server's `Option<bool>` exists to make: `null` is "the
    // range cannot answer this", which is not the same claim as "it is not
    // happening". A row reading "Not triggered" here would report safety that was
    // never measured.
    expect(within(row as HTMLElement).getByText('Not measured')).toBeInTheDocument()
    expect(within(row as HTMLElement).queryByText('Not triggered')).not.toBeInTheDocument()
  })

  it('renders a triggered tripwire and a clear one differently', async () => {
    setup()
    renderWithProviders(<AdminScreen />, { route: '/admin' })

    const triggered = (await screen.findByText('Free-tier cost over a quarter of revenue')).closest('li')
    const clear = screen.getByText('Conversion of free accounts under 3%').closest('li')
    expect(within(triggered as HTMLElement).getByText('Triggered')).toBeInTheDocument()
    expect(within(clear as HTMLElement).getByText('Not triggered')).toBeInTheDocument()
  })

  it('says an account has never been active rather than leaving the cell blank', async () => {
    setup()
    renderWithProviders(<AdminScreen />, { route: '/admin' })

    const row = (await screen.findByText('quiet@example.test')).closest('tr')
    expect(row).not.toBeNull()
    // `last_active_at: null` is "this account has never taken an action" — a real
    // state, and an empty cell would be indistinguishable from a render failure.
    expect(within(row as HTMLElement).getByText('Never active')).toBeInTheDocument()
    // And the zero counts are rendered as zeroes, because zero requests *is* the
    // measurement for this account — both of them, in their own cells.
    expect(within(row as HTMLElement).getAllByText('0')).toHaveLength(2)
  })

  it('shows a payment awaiting review with the server\'s own reason', async () => {
    setup()
    renderWithProviders(<AdminScreen />, { route: '/admin' })

    // The status is the gateway's string, unmodified: an operator reconciling
    // against the gateway's dashboard needs the two to match, so `review` is not
    // rendered as a friendlier word.
    expect(await screen.findByText('review')).toBeInTheDocument()
    expect(screen.getByText('amount mismatch')).toBeInTheDocument()
    expect(screen.getByText('ORD-2026-0001')).toBeInTheDocument()
  })

  it('computes the margin from the report rather than reading a blended figure', async () => {
    setup()
    renderWithProviders(<AdminScreen />, { route: '/admin' })

    // Rp 45.000 revenue − (USD 0.5625 × 16000 = Rp 9.000) = Rp 36.000, which is
    // 80%. The arithmetic is the screen's, and the rate it used is printed beside
    // it so a reader can check it.
    expect(await screen.findByText('80%')).toBeInTheDocument()
    // One amount, one rendering: the rate is a rupiah figure, so both places that
    // print it group it the Indonesian way. It was "Rp 16,000" on the summary card
    // and "Rp 16.000" in the margin section for the same number.
    expect(screen.getAllByText(/Rp 16\.000 per USD/).length).toBeGreaterThan(0)
    expect(screen.queryByText(/Rp 16,000/)).not.toBeInTheDocument()
  })

  it('says there is no margin when there is no revenue, rather than showing 0%', async () => {
    setup({
      margin: {
        ...MARGIN,
        months: [{ ...MARGIN.months[0], revenue_idr: 0, per_plan: [] }],
      },
    })
    renderWithProviders(<AdminScreen />, { route: '/admin' })

    // A percentage of nothing is unanswerable. `0%` would read as a measured
    // break-even, which is the one thing this screen must not imply about money.
    expect(await screen.findByText('No revenue')).toBeInTheDocument()
    expect(screen.queryByText('0%')).not.toBeInTheDocument()
  })

  it('states an empty month rather than rendering a table with no rows', async () => {
    setup({
      margin: { ...MARGIN, months: [{ ...MARGIN.months[0], per_plan: [] }] },
    })
    renderWithProviders(<AdminScreen />, { route: '/admin' })

    // The report includes a month with no rows on purpose, so it is a stated
    // absence; an empty table would read as a rendering bug.
    expect(await screen.findByText('No rows in this month.')).toBeInTheDocument()
  })

  it('gives every card a heading that does not restate its section', async () => {
    setup()
    const { container } = renderWithProviders(<AdminScreen />, { route: '/admin' })

    // The margin section is the last to resolve, so waiting on one of its tripwires puts
    // every card on screen before the outline is read.
    await screen.findByText('Two consecutive months at a loss')

    // A card heading never restates the section it sits in: the same words twice reads
    // as two figures where there is one. The section names the subject ("User activity"),
    // the card heading names what the card itself shows (the table, the window, the rate).
    const headings = container.querySelectorAll('h3')
    expect(headings.length).toBeGreaterThan(0)
    for (const heading of headings) {
      const sectionHeading = heading.closest('section')?.querySelector('h2')
      expect(heading.textContent).not.toBe(sectionHeading?.textContent)
    }
  })

  it('refuses on a phone, naming the reason, instead of showing a cramped table', async () => {
    installFakePlatform('ios')
    setAccessToken('access-1')
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, { ...ADMIN_USER, is_admin: true })
      return jsonResponse(404, { code: 'not_found', message: 'no' })
    })
    renderWithProviders(<AdminScreen />, { route: '/admin' })

    // Account data on a phone is data shown to whoever is holding the phone, and
    // the console is not designed for that width — so the refusal is the feature.
    expect(await screen.findByText('Not available on this device')).toBeInTheDocument()
    expect(screen.queryByText('active@example.test')).not.toBeInTheDocument()
  })
})
