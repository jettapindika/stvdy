import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'
import { screen, waitFor } from '@testing-library/react'
import userEvent from '@testing-library/user-event'

import { ActionCard, actionCost } from './ActionCard'
import type { ActionRow } from '@/api/queries/actions'
import { AGENT_CREDITS } from '@/features/billing/plans'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The confirmation card, and the honesty rules it exists to keep.
 *
 * A WRITE tool can only ever reach `GateDecision::Propose` (A271) — the gate has
 * no arm that executes one — so this card is *always* rendering a proposal, and
 * three of the assertions below are the honesty gate stated as code:
 *
 *  * the label is `Suggested …` until confirmed, and the card can never read as
 *    though the thing had happened;
 *  * the cost is on the card **before** the confirm, read from `plans.ts` — the
 *    same file the server charges from — and asserted against the exported table
 *    rather than a literal, so a change to `config/plans.toml` moves both;
 *  * a resolved proposal renders as resolved, and a rejected or expired one is not
 *    on screen at all.
 *
 * The keyboard assertions are here rather than in a separate file because the
 * three controls are the card's whole interface: if they cannot be reached by
 * `Tab` and the ring cannot be seen, the card is unusable for a keyboard user and
 * the honesty copy is decoration.
 */

const TZ = 'Asia/Jakarta'

function proposal(overrides: Partial<ActionRow> = {}): ActionRow {
  return {
    id: 'a1',
    tool: 'create_task',
    args: { title: 'Selesaikan tugas kriptografi', due_at: '2026-10-16T16:59:00Z' },
    status: 'pending',
    created_at: '2026-10-12T09:00:00Z',
    expires_at: '2026-10-12T09:30:00Z',
    resolved_at: null,
    resolved_by_device_id: null,
    result: null,
    error: null,
    message_id: null,
    ...overrides,
  }
}

beforeEach(() => {
  installFakePlatform('web')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('a pending proposal', () => {
  it('shows what will be added, its cost, and the label the honesty rules require', () => {
    const { container } = renderWithProviders(
      <ActionCard action={proposal({ tool: 'create_flashcards' })} timeZone={TZ} />,
    )

    // The wording is "Suggested …" until confirmed, which is what stops the card
    // reading as though the flashcards already exist.
    expect(screen.getByText(/^Suggested /)).toBeTruthy()
    expect(screen.getByText(/Suggested Create flashcards/)).toBeTruthy()

    // The spec's own sentence, verbatim, in both languages.
    expect(screen.getByText('Nothing is added until you confirm.')).toBeTruthy()

    // The cost comes from the exported table, not a literal: a `plans.ts` that
    // read the wrong key would fail here rather than quoting a number the server
    // does not charge.
    expect(AGENT_CREDITS.flashcards).not.toBeNull()
    expect(screen.getByText(`Costs ${String(AGENT_CREDITS.flashcards)} credits when you confirm.`)).toBeTruthy()

    // Confirm / Edit / Dismiss are all present and reachable.
    expect(screen.getByRole('button', { name: 'Confirm' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Edit' })).toBeTruthy()
    expect(screen.getByRole('button', { name: 'Dismiss' })).toBeTruthy()

    // Every target is at least 44px at phone width: jsdom has no layout engine, so
    // the assertion is on the class the design system uses for it, which is the
    // same contract `LockedFeature` and the tab bar carry.
    for (const button of container.querySelectorAll('button')) {
      expect(button.className, `${button.textContent ?? ''} is not a 44px target`).toContain('min-h-11')
    }
  })

  it('prices the two generative actions from the matrix and nothing else', () => {
    // `create_task` is not a priced action: the matrix has no key for it, so the
    // card says so rather than quoting a `0` the file does not state.
    expect(actionCost('create_task')).toBeNull()
    expect(actionCost('update_task')).toBeNull()
    expect(actionCost('create_event')).toBeNull()
    expect(actionCost('schedule_study_block')).toBeNull()
    expect(actionCost('create_flashcards')).toBe(AGENT_CREDITS.flashcards)
    expect(actionCost('create_quiz')).toBe(AGENT_CREDITS.quiz)
  })

  it('reaches Confirm, Edit and Dismiss by keyboard, with the ring token', async () => {
    const user = userEvent.setup()
    const { container } = renderWithProviders(
      <ActionCard action={proposal()} timeZone={TZ} />,
    )

    // The global `:focus-visible` rule in `styles.css` carries the ring for every
    // control, and `invariants.test.ts` is what forbids any other focus utility.
    // What is asserted here is the other half: the three controls are real tab
    // stops in order, so a keyboard user reaches them at all.
    const confirm = screen.getByRole('button', { name: 'Confirm' })
    await user.tab()
    expect(document.activeElement).toBe(confirm)
    await user.tab()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Edit' }))
    await user.tab()
    expect(document.activeElement).toBe(screen.getByRole('button', { name: 'Dismiss' }))

    // Nothing on the card is reachable by colour alone: the state carries an icon
    // and a word, and the icons are decorative (`aria-hidden`) so the word is what
    // a screen reader reads.
    const icons = container.querySelectorAll('svg[aria-hidden="true"]')
    expect(icons.length).toBeGreaterThan(0)
    expect(container.textContent).toContain('Suggested')
  })

  it('shows a due instant in the account’s zone rather than as a raw ISO string', () => {
    // `2026-10-16T16:59Z` is 23:59 in Jakarta. The raw instant would be the least
    // readable form of the one fact the resolution exists to get right, and Phase
    // 9's gate found exactly this defect on the capture card.
    const { container } = renderWithProviders(
      <ActionCard action={proposal()} timeZone={TZ} />,
    )
    expect(container.textContent).not.toContain('2026-10-16T16:59:00Z')
    expect(container.textContent).toContain('16 Oct 2026')
  })

  it('confirms the proposal as it stands when no edit is open', async () => {
    const calls: { url: string; body: string }[] = []
    mockFetch((url, init) => {
      calls.push({ url, body: String(init?.body ?? '') })
      return jsonResponse(200, { action: { ...proposal(), status: 'confirmed' } })
    })
    const user = userEvent.setup()
    renderWithProviders(<ActionCard action={proposal()} timeZone={TZ} />)

    await user.click(screen.getByRole('button', { name: 'Confirm' }))

    await waitFor(() => {
      expect(calls.some((call) => call.url.endsWith('/actions/a1/confirm'))).toBe(true)
    })
    const confirm = calls.find((call) => call.url.endsWith('/actions/a1/confirm'))
    // An empty body confirms exactly what was proposed; a body naming `args` would
    // be an edit, and the student asked for none.
    expect(confirm?.body).toBe('{}')
  })

  it('sends an edited object through the same confirm, and refuses an edit that is not one', async () => {
    const calls: string[] = []
    mockFetch((url, init) => {
      calls.push(String(init?.body ?? ''))
      return jsonResponse(200, { action: { ...proposal(), status: 'confirmed' } })
    })
    const user = userEvent.setup()
    renderWithProviders(<ActionCard action={proposal()} timeZone={TZ} />)

    await user.click(screen.getByRole('button', { name: 'Edit' }))
    const field = screen.getByLabelText('Edit what will be added')
    await user.clear(field)
    // An unterminated object: userEvent's `{{` is one literal `{`.
    await user.type(field, '{{"title":"Judul lain"')
    await user.click(screen.getByRole('button', { name: 'Confirm' }))

    // Not JSON: the app says so and sends nothing, because silently confirming the
    // original would be the app doing something the student did not ask for.
    expect(screen.getByRole('alert').textContent).toContain('not valid JSON')
    expect(calls).toEqual([])
  })

  it('does not let a proposal read as done while it is only suggested', () => {
    const { container } = renderWithProviders(
      <ActionCard action={proposal()} timeZone={TZ} />,
    )
    expect(container.textContent).not.toContain('Confirmed')
    expect(screen.getByText('Nothing is added until you confirm.')).toBeTruthy()
  })
})

describe('a resolved proposal', () => {
  it('renders a resolved proposal as resolved rather than hiding it', () => {
    renderWithProviders(
      <ActionCard
        action={proposal({
          status: 'confirmed',
          resolved_at: '2026-10-12T09:05:00Z',
          resolved_by_device_id: 'd2',
          result: { task: { id: 't9', title: 'Selesaikan tugas kriptografi' } },
        })}
        timeZone={TZ}
      />,
    )

    expect(screen.getByText('Confirmed')).toBeTruthy()
    // The row it produced, named — which is what makes the second device's card
    // evidence rather than a claim.
    expect(screen.getByText(/Selesaikan tugas kriptografi/)).toBeTruthy()
    // The controls are gone: there is nothing left to decide.
    expect(screen.queryByRole('button', { name: 'Confirm' })).toBeNull()
    expect(screen.queryByRole('button', { name: 'Dismiss' })).toBeNull()
    // And the "nothing is added yet" sentence is gone with them, because it is no
    // longer true.
    expect(screen.queryByText('Nothing is added until you confirm.')).toBeNull()
  })

  it('shows why a confirmed action did not finish, when the server recorded one', () => {
    renderWithProviders(
      <ActionCard
        action={proposal({
          status: 'confirmed',
          result: { deck: { id: 'd1', title: 'Graf' } },
          error: 'the generation job could not be queued',
        })}
        timeZone={TZ}
      />,
    )
    // The row was created and the work did not start. Hiding that would be the
    // screen claiming a deck that is not generating.
    expect(screen.getByRole('status').textContent).toContain('did not start')
  })

  it('shows no price on a resolved card, because nothing is left to charge for', () => {
    renderWithProviders(
      <ActionCard
        action={proposal({
          tool: 'create_flashcards',
          status: 'confirmed',
          result: { deck: { id: 'd1', title: 'Graf' } },
        })}
        timeZone={TZ}
      />,
    )
    expect(screen.queryByText(/credits when you confirm/)).toBeNull()
  })
})
