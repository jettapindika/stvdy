import { screen, within } from '@testing-library/react'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { PlanUsageScreen } from './PlanUsageScreen'
import { formatCount } from './CreditMeter'
import i18n from '@/i18n'
import { setAccessToken } from '@/lib/tokens'
import { formatInZone } from '@/lib/time'
import { fixtureNumbers, installFakePlatform } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { PERIOD_END, RESETS_AT, USAGE, USER, serveUsage } from '@/test/usage'

/**
 * The plan and usage screen against a scripted `GET /usage`.
 *
 * The fixture is shared with `MobilePlanScreen.test.tsx` (see `@/test/usage`):
 * both screens render the same body, so a second hand-written payload would be a
 * second answer to what the wire shape is.
 *
 * The honesty test is the load-bearing one. Asserting "the numbers are shown" is
 * satisfied by a screen that hardcodes them; asserting that **no number appears
 * which the payload does not contain** is what makes a literal, a stale cache, or
 * a locally computed total fail. Every other test here is about a state the
 * screen can be in.
 */

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(async () => {
  vi.unstubAllGlobals()
  await i18n.changeLanguage('en')
})

/** The digit runs a string contains, with the grouping separators removed. */
function digitRuns(text: string): string[] {
  return text.match(/[0-9]+/g) ?? []
}

/**
 * The rows of a `<dl>`, keyed by the label the user reads.
 *
 * Scoped this way rather than by a global text search so an assertion about one
 * row cannot be satisfied by a number that happens to appear in another.
 */
function row(label: string): HTMLElement {
  const term = screen.getByText(label)
  const group = term.closest('div')
  if (group === null) throw new Error(`no row for ${label}`)
  return group
}

describe('the plan and usage screen', () => {
  it('renders only the numbers the usage endpoint returned', async () => {
    serveUsage()
    const { container } = renderWithProviders(<PlanUsageScreen />)

    // The plan, and the buckets with their total, as the ledger holds them.
    expect(await screen.findByText('Free')).toBeInTheDocument()
    expect(within(row('Monthly grant')).getByText('60')).toBeInTheDocument()
    expect(within(row('Purchased')).getByText('12')).toBeInTheDocument()
    expect(within(row('Total')).getByText('72')).toBeInTheDocument()

    // Each limit as `used of limit`, with the ceiling alone where there is no
    // counter, and "Unlimited" where the plan does not cap the axis.
    expect(within(row('Workspaces')).getByText('2 of 2')).toBeInTheDocument()
    expect(within(row('Sources')).getByText('3 of 20')).toBeInTheDocument()
    expect(within(row('Largest file (MB)')).getByText('20')).toBeInTheDocument()
    // Storage is bytes held against a megabyte ceiling, and the byte count is the
    // server's rather than a rounded megabyte figure.
    expect(within(row('Storage')).getByText('1,048,576 bytes of 200 MB')).toBeInTheDocument()
    expect(within(row('Chat messages per day')).getByText('4 of 20')).toBeInTheDocument()
    expect(within(row('Google accounts')).getByText('0')).toBeInTheDocument()

    // The cost table, from the payload's `costs` block.
    expect(within(row('Chat message')).getByText('1')).toBeInTheDocument()
    expect(within(row('Report outline')).getByText('3')).toBeInTheDocument()
    expect(within(row('Report section')).getByText('6')).toBeInTheDocument()
    expect(within(row('Syllabus import')).getByText('8')).toBeInTheDocument()

    // A reached ceiling says so in words, so the state is not carried by tone.
    expect(within(row('Workspaces')).getByText('At the limit')).toBeInTheDocument()

    // The period end, as a date in the account's own zone rather than an instant.
    const periodEnd = formatInZone(new Date(PERIOD_END), USER.timezone, { dateStyle: 'medium' })
    expect(screen.getByText(`Current period ends ${periodEnd}`)).toBeInTheDocument()

    // **The other half: no number the fixture does not contain.** Every digit run
    // on the screen has to trace to a number the endpoint sent (formatted the way
    // the app formats numbers) or to the date the endpoint's instant renders as.
    const allowed = new Set([
      ...fixtureNumbers(USAGE).flatMap((value) => digitRuns(formatCount(value, 'en'))),
      ...digitRuns(periodEnd),
      ...digitRuns(formatInZone(new Date(RESETS_AT), USER.timezone, { dateStyle: 'medium' })),
    ])
    const shown = digitRuns(container.textContent ?? '')
    expect(shown.filter((run) => !allowed.has(run))).toEqual([])

    // And nothing prints an instant as it arrived on the wire.
    expect(container.textContent).not.toMatch(/\d{4}-\d{2}-\d{2}T/)
  })

  it('shows the credit cost of every generative action', async () => {
    serveUsage()
    renderWithProviders(<PlanUsageScreen />)

    // The four actions a student spends on without thinking about it, each with
    // the price the plan matrix charges for it. These are the numbers the panels
    // put on their own buttons before the action runs, and this screen is where
    // they can be checked against the account's balance in one place.
    await screen.findByText('Free')
    const costs: [string, string][] = [
      ['Chat message', '1'],
      ['Web search', '2'],
      ['A batch of flashcards', '4'],
      ['A set of quiz questions', '6'],
      ['Report outline', '3'],
      ['Report section', '6'],
      ['Task breakdown', '2'],
      ['Syllabus import', '8'],
      ['Study plan', '2'],
      ['Assistant step', '2'],
    ]
    for (const [label, credits] of costs) {
      expect(within(row(label)).getByText(credits), label).toBeInTheDocument()
    }

    // The long-document surcharge is quoted from the same block rather than
    // restated: `20,000` tokens above which a document costs `1` more credit per
    // further thousand.
    expect(
      screen.getByText(
        'Documents longer than 20,000 tokens cost 1 more credit for each thousand tokens after that.',
      ),
    ).toBeInTheDocument()
  })

  it('states the locked format with no upgrade CTA on a mobile platform', async () => {
    serveUsage()
    installFakePlatform('ios')
    renderWithProviders(<PlanUsageScreen />)

    // The free plan includes Markdown and not PDF, so the PDF row is the locked
    // one. On a phone it states the fact: no price, no button, no link (A143).
    expect(await screen.findByText('PDF')).toBeInTheDocument()
    expect(screen.getByText('Not available on your plan')).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: /Upgrade/ })).not.toBeInTheDocument()
    expect(screen.queryByText(/Rp/)).not.toBeInTheDocument()
    expect(screen.queryByText(/upgrade to/i)).not.toBeInTheDocument()
  })

  it('names the plan that has the locked format on desktop, with no price', async () => {
    serveUsage()
    installFakePlatform('macos')
    renderWithProviders(<PlanUsageScreen />)

    // The plan that includes PDF comes from `config/plans.toml` through the same
    // reader the export menu uses, rather than being written into the screen.
    expect(await screen.findByText('Available on Pro')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Upgrade to Pro' })).toHaveAttribute(
      'href',
      '/billing',
    )
    // Still no price and no checkout: purchase UI is Phase 13, and
    // `config/billing_policy.toml` governs whether it may exist at all.
    expect(screen.queryByText(/Rp/)).not.toBeInTheDocument()
  })

  it('renders the paused ceiling as a sentence naming the reset date', async () => {
    serveUsage({
      ...USAGE,
      provider_cost: { paused: true, resets_at: RESETS_AT },
    })
    renderWithProviders(<PlanUsageScreen />)

    const resets = formatInZone(new Date(RESETS_AT), USER.timezone, { dateStyle: 'medium' })
    expect(
      await screen.findByText(
        `New generation is paused until ${resets}. This account has reached the ceiling its plan sets for provider costs this month.`,
      ),
    ).toBeInTheDocument()
  })

  it('reads every row in Indonesian', async () => {
    serveUsage()
    await i18n.changeLanguage('id')
    renderWithProviders(<PlanUsageScreen />)

    // The plan's own name is a proper noun from `config/plans.toml` and does not
    // change; the labels around it do, which is the half that used to be English
    // for an Indonesian reader.
    expect(await screen.findByText('Free')).toBeInTheDocument()
    expect(screen.getByText('Paket dan pemakaian')).toBeInTheDocument()
    expect(within(row('Jatah bulanan')).getByText('60')).toBeInTheDocument()
    expect(within(row('Ruang belajar')).getByText('2 dari 2')).toBeInTheDocument()
    expect(within(row('Ruang belajar')).getByText('Sudah di batas')).toBeInTheDocument()
    expect(screen.getByRole('heading', { name: 'Biaya setiap tindakan' })).toBeInTheDocument()
    expect(screen.getByText('Termasuk di paketmu: Markdown')).toBeInTheDocument()
  })
})
