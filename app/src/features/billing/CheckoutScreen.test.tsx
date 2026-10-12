import { screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { CheckoutScreen } from './CheckoutScreen'
import i18n from '@/i18n'
import { setAccessToken } from '@/lib/tokens'
import { PAYMENT, PRO_MONTHLY, serveBilling } from '@/test/billing'
import { installFakePlatform, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import type { PaymentView } from '@/api/queries/billing'

/**
 * The checkout screen, against a scripted poll.
 *
 * Two properties carry this file, and both are support-ticket-shaped.
 *
 * **The total is the server's.** The fixture charges `6172` on a list price of
 * `12345`, and a third of the tests here assert that the number on screen is the
 * row's `amount_idr` — a screen that re-derived the arithmetic would print `6172`
 * too in this fixture, so one case is planted with a deliberately inconsistent row
 * (`amount_idr` that does not follow from the discount) to make the difference
 * observable. That is the only way this assertion can fail, and it must be able to.
 *
 * **Every state is spoken.** Five of the six states plus `offline` are rendered and
 * asserted for their *words*, because a QR screen is the natural place for a status
 * that exists only as a colour, and gate 5(d) forbids it. The state's own sentence
 * is asserted, not the chip's tone.
 *
 * The countdown is not asserted here for its arithmetic — `checkout.test.ts` does
 * that, with `now` injected, at the boundary. What is asserted here is that the
 * screen renders the server's expiry rather than a duration of its own, and that a
 * paid payment shows no QR.
 */

const PAYMENT_ID = PAYMENT.id

beforeEach(() => {
  // The screen decides whether the fixture's invoice is still live by reading the
  // **wall clock**, and `PAYMENT.expires_at` is a fixed timestamp, so the three
  // cases below that render the live state pass until that instant and fail from
  // it onwards. That is not a hypothesis: this suite was green in CI at 09:04Z on
  // 2026-10-12 and red from 09:15Z, the fixture's own expiry, with "Unable to find
  // an element with the text: Waiting for confirmation".
  //
  // So the clock is pinned instead — ten minutes inside the fixture's window,
  // *derived from the fixture* rather than written down twice, because two
  // constants that have to stay ordered are the same defect one step out.
  //
  // Only `Date` is faked. The screen polls on an interval, and faking
  // `setTimeout`/`setInterval` too would put testing-library's own async utilities
  // on a clock nothing advances.
  vi.useFakeTimers({
    toFake: ['Date'],
    now: Date.parse(PAYMENT.expires_at) - 10 * 60_000,
  })
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(async () => {
  vi.useRealTimers()
  vi.unstubAllGlobals()
  await i18n.changeLanguage('en')
})

/** Renders the screen on the checkout route for the fixture's payment. */
function renderCheckout(payment: PaymentView = PAYMENT) {
  serveBilling({ payment })
  return renderWithProviders(<CheckoutScreen paymentId={PAYMENT_ID} />)
}

/** The QR, if the screen is rendering one. */
function qr(container: HTMLElement): SVGElement | null {
  return container.querySelector('svg[role="img"]')
}

/**
 * The state region, once it says `text`.
 *
 * Found through the state's own sentence rather than by `findByRole('status')`,
 * because `SkeletonRows` is also a live region: a query by role alone would find
 * the loading state and assert against the wrong element.
 */
async function stateRegion(text: string): Promise<HTMLElement> {
  const found = await screen.findByText(text)
  const region = found.closest('[role="status"]')
  if (region === null) throw new Error(`no state region around ${text}`)
  return region as HTMLElement
}

describe('the checkout screen', () => {
  it('renders the server\'s amount and its itemisation, not a computed total', async () => {
    // The row's numbers do not follow from each other on purpose: `amount_idr` is
    // 5555 where a 50% discount on 12345 would be 6172. A screen that re-derived
    // the total would print 6172 here and fail, which is the whole point.
    const inconsistent: PaymentView = {
      ...PAYMENT,
      amount_idr: 5555,
      amount_before_discount_idr: PRO_MONTHLY,
      discount_percent: 50,
    }
    const { container } = renderCheckout(inconsistent)

    await screen.findByText('Total')
    expect(container.textContent).toMatch(/5,555/)
    expect(container.textContent).not.toMatch(/6,172/)
    // And the itemisation is the row's own fields, labelled as the row's facts.
    expect(screen.getByText('Price before discount')).toBeInTheDocument()
    // The element's whole text, not a substring: the row's value is the formatted
    // amount with its currency, so the matcher is the formatted string.
    expect(screen.getByText(/12,345/)).toBeInTheDocument()
    expect(screen.getByText('Student discount')).toBeInTheDocument()
    expect(screen.getByText('50%')).toBeInTheDocument()
  })

  it('renders the QR from the server\'s payload, verbatim', async () => {
    const { container } = renderCheckout()

    // The code appears only while the invoice is live, and it is drawn from
    // `qr_string` — the gateway's own payload — rather than from anything this app
    // composed. A QR the app invented would be one the gateway does not recognise.
    await screen.findByText('Waiting for confirmation')
    const code = qr(container)
    expect(code).not.toBeNull()
    expect(code?.getAttribute('aria-label')).toBe('QRIS code for this payment')
    // A real module grid, not an empty path: the payload encoded to something.
    expect(code?.querySelector('path')?.getAttribute('d')?.length ?? 0).toBeGreaterThan(100)
  })

  it('a paid payment never renders a new QR', async () => {
    const { container } = renderCheckout({ ...PAYMENT, status: 'paid', paid_at: '2026-10-12T09:02:00Z' })

    // Paid, and the code is gone even though the row still carries `qr_string` —
    // a second scan of a dead code is how a student pays twice.
    expect(await screen.findByText('Payment received')).toBeInTheDocument()
    expect(qr(container)).toBeNull()
    expect(PAYMENT.qr_string).not.toBeNull()
  })

  it('an expired invoice offers a new code and a review does not', async () => {
    const expired = renderCheckout({ ...PAYMENT, status: 'expired' })
    expect(await screen.findByText('This code has expired')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Get a new code' })).toBeInTheDocument()
    // The state says no payment was confirmed rather than claiming one was not
    // made — we cannot see the student's bank, only what reached us.
    expect(screen.getByText(/No payment has been confirmed for this code/)).toBeInTheDocument()
    expired.container.remove()

    // The assertions are scoped to the review render rather than to the document,
    // which is what makes them about *this* state: the expired screen above is
    // torn down, but a global query could still be satisfied by anything else the
    // shell rendered, and the claim being made is that the review state itself
    // offers no way to pay again.
    const review = renderCheckout({ ...PAYMENT, status: 'review' })
    expect(await screen.findByText('We are checking this payment')).toBeInTheDocument()
    expect(within(review.container).queryByRole('button', { name: 'Get a new code' })).toBeNull()
    // The one instruction that matters on a mismatch: do not pay twice.
    expect(within(review.container).getByText(/Do not pay again/)).toBeInTheDocument()
  })

  it('says what "we have not heard from the gateway yet" means', async () => {
    renderCheckout()

    // The state a student sits in when a webhook is lost. It must read as neither
    // failure nor success, and it must say what happens next.
    const status = await stateRegion('Waiting for confirmation')
    expect(within(status).getByText('Waiting for confirmation')).toBeInTheDocument()
    expect(within(status).getByText('Waiting')).toBeInTheDocument()
    expect(within(status).getByText(/We have not received confirmation yet/)).toBeInTheDocument()
    expect(within(status).getByText(/your plan starts as soon as the payment is confirmed/)).toBeInTheDocument()
    // Not the failure headline and not the success headline. The check is on the
    // two sentences a student would act on rather than on the words "failed" and
    // "received", because the honest detail legitimately says "we have not received
    // confirmation yet" — and a matcher that failed on that would be a test that
    // forbids the sentence this state exists to say.
    expect(status.textContent).not.toContain('Payment received')
    expect(status.textContent).not.toContain('The payment did not go through')
    expect(status.textContent).not.toContain('Your plan is active')
  })

  it('distinguishes a dropped connection from a failed payment', async () => {
    // A request that never reached the server is `offline`, and the copy says the
    // code may still be valid — telling a student their payment failed because
    // their Wi-Fi dropped is a lie they might act on.
    mockFetch(() => {
      throw new TypeError('Failed to fetch')
    })
    renderWithProviders(<CheckoutScreen paymentId={PAYMENT_ID} />)

    const status = await stateRegion('We could not reach Stvdy')
    expect(within(status).getByText('We could not reach Stvdy')).toBeInTheDocument()
    expect(within(status).getByText(/may still be valid/)).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument()
  })

  it('renders the failed state, which offers no retry on this screen', async () => {
    renderCheckout({ ...PAYMENT, status: 'failed' })

    expect(await screen.findByText('The payment did not go through')).toBeInTheDocument()
    // The remedy is a different invoice, from the plans page: a retry here would
    // re-mint the same row.
    expect(screen.queryByRole('button', { name: 'Get a new code' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Try again' })).toBeNull()
  })

  it('names the order, so a student can quote it to support', async () => {
    renderCheckout()

    expect(await screen.findByText(/Order stvdy_0f1e2d3c/)).toBeInTheDocument()
  })

  it('offers no purchase surface on a platform whose policy forbids one', async () => {
    // The route is registered unconditionally — the table is evaluated at module
    // scope, before a platform exists — so a phone build has a
    // `/billing/checkout/:id` URL that must not present a purchase screen when a
    // reviewer types it. Nothing on mobile links here and the deep-link classifier
    // has no billing path, so this is the check that keeps it that way, and it is
    // the defect P14AbsencePhone found: the screen rendered the amount, the QR and
    // the countdown on `ios` because it never consulted `billing_policy`.
    installFakePlatform('ios')
    // The route's own answer is planted: if the screen reached the poll at all,
    // this payment would render. The assertion is that it never gets that far.
    const { container } = renderCheckout()

    expect(await screen.findByText(/does not sell subscriptions/)).toBeInTheDocument()
    expect(container.textContent).not.toMatch(/Rp\u00a0|IDR\u00a0/)
    expect(container.querySelectorAll('button, a')).toHaveLength(0)
    expect(qr(container)).toBeNull()
    // And no request was made for the invoice, because there is nothing to show.
    expect(container.textContent).not.toContain('Waiting for confirmation')
  })

  it('reads the whole screen in Indonesian, including the state', async () => {
    await i18n.changeLanguage('id')
    renderCheckout()

    // The money copy is the copy that matters most in a product sold in
    // Indonesia, and the state's sentence is part of it.
    expect(await screen.findByText('Menunggu konfirmasi')).toBeInTheDocument()
    expect(screen.getByText('Total')).toBeInTheDocument()
    // The non-breaking space the currency formatter writes is normalised to a
    // plain space by the testing library's default text normaliser, so the
    // matcher is written with one — the assertion is about the number and the
    // symbol, and `checkout.test.ts` is what pins the exact character.
    expect(screen.getByText(/Rp 12.345/)).toBeInTheDocument()
  })
})
