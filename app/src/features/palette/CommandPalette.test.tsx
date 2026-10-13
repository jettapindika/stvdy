import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { beforeEach, describe, expect, it, vi } from 'vitest'

import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { CommandPalette } from './CommandPalette'

/**
 * The palette's keyboard contract, which is the only way its rows are reachable.
 *
 * There is nothing to Tab to: focus never leaves the input, and the active row is
 * marked with `aria-activedescendant` — an attribute a screen reader exposes **only**
 * on a combobox whose listbox is named by `aria-controls` and whose options are
 * `role="option"`. The markup shipped with `aria-activedescendant` on a plain
 * `<input>` over a list of `<button aria-current>`, so the attribute was inert: the
 * highlight moved and the screen reader said nothing. That is what these cases
 * assert, because no other test in the suite renders this component at all.
 *
 * `scrollIntoView` is the second half of the same problem: with focus in the input,
 * nothing scrolls the list, and the list is `overflow-y-auto` inside a `70vh` popup.
 * jsdom has no `scrollIntoView` at all, so the stub is what lets the assertion exist.
 */

const WORKSPACE = {
  id: 'w1',
  name: 'Struktur Data',
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
  source_count: 1,
  course_code: 'CS 401',
  color: 0,
  exam_date: null,
}

const TASK = {
  id: 't1',
  workspace_id: 'w1',
  parent_id: null,
  title: 'Latihan graf',
  notes: null,
  due_at: null,
  estimate_min: null,
  priority: 2,
  status: 'todo',
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
  deleted_at: null,
}

const SOURCE = {
  id: 's1',
  workspace_id: 'w1',
  kind: 'pdf',
  title: 'Bab 3 Graf',
  url: null,
  file_size: 2048,
  mime: 'application/pdf',
  status: 'ready',
  error: null,
  created_at: '2026-10-11T08:00:00Z',
}

const DECK = {
  id: 'd1',
  workspace_id: 'w1',
  title: 'Dek Graf',
  source_ids: ['s1'],
  status: 'ready',
  error: null,
  requested_count: 10,
  card_count: 2,
  created_at: '2026-10-11T08:00:00Z',
}

const notFound = () =>
  jsonResponse(404, {
    code: 'not_found',
    message: 'no such route',
    min_plan: null,
    limit: null,
    used: null,
  })

/** The account's data, scripted so the index has every group in it. */
function mockAccount(): void {
  mockFetch((url) => {
    const path = new URL(url).pathname
    if (path === '/workspaces') return jsonResponse(200, { workspaces: [WORKSPACE] })
    if (path === '/tasks') return jsonResponse(200, { tasks: [TASK] })
    if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [SOURCE] })
    if (path === '/workspaces/w1/decks') return jsonResponse(200, { decks: [DECK] })
    return notFound()
  })
}

beforeEach(() => {
  // The web platform, not a native one: `useTasks` reads through the local cache
  // (`platform.sync`) whenever the runtime is not `web`, so on a native fake the
  // task row would come from the cache fake rather than from the scripted `/tasks`
  // answer, and the index under test would be missing a group.
  installFakePlatform('web')
  mockAccount()
})

describe('the command palette', () => {
  it('exposes its rows as listbox options under a combobox', async () => {
    const { container } = renderWithProviders(<CommandPalette open onClose={() => {}} />)

    // The rows are loaded, so the index is the real one rather than an empty list
    // that would pass every assertion below vacuously.
    expect(await screen.findByRole('option', { name: /Bab 3 Graf/ })).toBeInTheDocument()

    const input = screen.getByRole('combobox')
    const listbox = screen.getByRole('listbox')
    expect(input).toHaveAttribute('aria-controls', listbox.id)

    // `aria-activedescendant` must name an element that is in the listbox, or the
    // attribute points nowhere and the reader announces nothing.
    const active = input.getAttribute('aria-activedescendant')
    expect(active).not.toBeNull()
    expect(listbox.contains(container.querySelector(`#${CSS.escape(active ?? '')}`))).toBe(true)

    const options = within(listbox).getAllByRole('option')
    expect(options).toHaveLength(9) // 5 commands + course + source + deck + task
    expect(options.filter((option) => option.getAttribute('aria-selected') === 'true')).toHaveLength(
      1,
    )
  })

  it('moves the active option with the arrow keys and scrolls it into view', async () => {
    const user = userEvent.setup()
    const scrollIntoView = vi.fn()
    // jsdom has no `scrollIntoView` on an element, and the palette calls it through
    // the ref rather than through focus, so this spy is the only observer.
    Element.prototype.scrollIntoView = scrollIntoView

    renderWithProviders(<CommandPalette open onClose={() => {}} />)
    const input = screen.getByRole('combobox')
    await screen.findByRole('option', { name: /Bab 3 Graf/ })

    // The active row is read back through the attribute rather than by index, so the
    // assertion is about the contract a screen reader gets: whatever id the input
    // points at must be a real option, and exactly one must be marked selected.
    const activeOption = () =>
      screen.getAllByRole('option').find((option) => option.id === input.getAttribute('aria-activedescendant'))
    const selected = () =>
      screen
        .getAllByRole('option')
        .filter((option) => option.getAttribute('aria-selected') === 'true')

    expect(selected()).toHaveLength(1)
    const first = activeOption()
    expect(first).toBe(selected()[0])

    await user.click(input)
    await user.keyboard('{ArrowDown}')
    const second = activeOption()
    expect(second).not.toBe(first)
    expect(second).toBe(selected()[0])
    expect(scrollIntoView).toHaveBeenCalled()
    // `nearest` is the scroll that leaves an already-visible row alone; without it
    // every keystroke would jump the list to centre the row.
    expect(scrollIntoView).toHaveBeenLastCalledWith({ block: 'nearest' })
  })

  it('activates the row the reader was told about, and closes on Enter', async () => {
    const user = userEvent.setup()
    const onClose = vi.fn()
    renderWithProviders(<CommandPalette open onClose={onClose} />)

    const input = screen.getByRole('combobox')
    await screen.findByRole('option', { name: /Bab 3 Graf/ })

    await user.click(input)
    await user.keyboard('{ArrowDown}')
    const active = input.getAttribute('aria-activedescendant')
    const row = screen.getAllByRole('option').find((option) => option.id === active)
    expect(row?.textContent).toContain('Workspaces')

    await user.keyboard('{Enter}')
    expect(onClose).toHaveBeenCalledTimes(1)
  })
})
