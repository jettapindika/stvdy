import { describe, expect, it, vi, beforeEach, afterEach } from 'vitest'

import { ChatScreen } from '@/features/chat/ChatScreen'
import { StudyScreen } from '@/features/study/StudyScreen'
import { TodayScreen } from '@/features/today/TodayScreen'
import { WorkspaceListScreen } from '@/features/workspaces/WorkspaceListScreen'
import { WorkspaceScreen } from '@/features/workspaces/WorkspaceScreen'
import { installFakePlatform, jsonResponse, mockFetch } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { expectNoA11yViolations } from '@/test/a11y'
import { setAccessToken } from '@/lib/tokens'

/**
 * The accessibility gate on the real screens, not just the gallery.
 *
 * The gallery test proves the component library is sound in isolation. It cannot
 * prove a *screen* is: the defects axe finds on a screen are the ones produced by
 * composition — a heading level skipped because a panel starts at `h3`, an input
 * whose label is a sibling instead of a `<label for>`, a region with no accessible
 * name. Those only exist once the pieces are assembled, so each screen is rendered
 * with its real data path and checked.
 *
 * axe is a floor and says nothing about whether a screen makes sense to a
 * screen-reader user; the keyboard and focus behaviour is asserted in the
 * per-screen test files, where it belongs.
 */

/**
 * Checks a screen, and fails if the screen rendered nothing.
 *
 * An empty container passes every accessibility rule, so a check without this
 * would go green the moment a screen stopped rendering — which is the failure mode
 * that looks least like a failure. The positive check is supplied by the caller
 * rather than assumed, because a panel rendered on its own has no heading: the
 * heading belongs to the screen that hosts it, so asserting one here would fail
 * for the wrong reason.
 */
async function expectAccessible(container: HTMLElement, rendered: () => void): Promise<void> {
  await expectNoA11yViolations(container)
  rendered()
}

/** The default positive check: the screen drew a heading and some prose. */
function heading(container: HTMLElement, what: string): void {
  const headings = container.querySelectorAll('h1, h2, h3')
  expect(headings.length, `${what} rendered no heading`).toBeGreaterThan(0)
  expect(container.textContent?.trim().length ?? 0, `${what} rendered no text`).toBeGreaterThan(20)
}

const WORKSPACE = {
  id: 'w1',
  name: 'Struktur Data',
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
  source_count: 1,
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
  title: 'Graf',
  source_ids: ['s1'],
  status: 'ready',
  error: null,
  requested_count: 10,
  card_count: 2,
  created_at: '2026-10-11T08:00:00Z',
}

/** A 404 for anything a screen asks for that this test does not script. */
const notFound = () =>
  jsonResponse(404, {
    code: 'not_found',
    message: 'no such route',
    min_plan: null,
    limit: null,
    used: null,
  })

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('screen accessibility', () => {
  it('Today has no axe violations', async () => {
    mockFetch(() => notFound())
    const { container } = renderWithProviders(<TodayScreen />, { route: '/' })
    await expectAccessible(container, () => heading(container, 'Today'))
  })

  it('the workspace list has no axe violations', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/workspaces') return jsonResponse(200, { workspaces: [WORKSPACE] })
      return notFound()
    })
    const { container } = renderWithProviders(<WorkspaceListScreen />, { route: '/workspaces' })
    await expectAccessible(container, () => heading(container, 'the workspace list'))
  })

  it('the workspace, on its sources panel, has no axe violations', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [SOURCE] })
      if (path === '/workspaces/w1/messages') return jsonResponse(200, { messages: [] })
      if (path === '/workspaces/w1/decks') return jsonResponse(200, { decks: [DECK] })
      return notFound()
    })
    // The workspace screen takes its id from the route, so the route is the prop.
    const { container } = renderWithProviders(<WorkspaceScreen />, { route: '/workspaces/w1' })
    await expectAccessible(container, () => heading(container, 'the workspace'))
  })

  it('the chat composer has no axe violations', async () => {
    // The composer is the screen the spec makes the most accessibility demands of:
    // two labelled toggles, a described hint, and the honesty line.
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [SOURCE] })
      if (path === '/workspaces/w1/messages') return jsonResponse(200, { messages: [] })
      return notFound()
    })
    const { container } = renderWithProviders(<ChatScreen workspaceId="w1" />, {
      route: '/workspaces/w1',
    })
    await expectAccessible(container, () => {
      // The panel has no heading of its own; its landmark is the composer, which
      // the spec requires to be labelled and described.
      const composer = container.querySelector('textarea#chat-message')
      expect(composer, 'the composer is missing').not.toBeNull()
      expect(composer?.getAttribute('aria-describedby')).toBe('chat-message-hint')
      expect(container.textContent).toContain('AI can make mistakes. Check the citations.')
    })
  })

  it('the flashcard review has no axe violations', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/decks/d1') {
        return jsonResponse(200, { deck: { ...DECK, cards: [] } })
      }
      if (path === '/decks/d1/due') {
        return jsonResponse(200, {
          cards: [
            {
              id: 'c1',
              deck_id: 'd1',
              front: 'Apa itu graf?',
              back: 'Kumpulan simpul dan sisi.',
              chunk_id: null,
              starred: false,
              flagged: false,
              ease: 2.5,
              interval_days: 0,
              due_at: '2026-10-11T08:00:00Z',
              created_at: '2026-10-11T08:00:00Z',
            },
          ],
          next_due_at: null,
        })
      }
      return notFound()
    })
    const { container } = renderWithProviders(<StudyScreen />, { route: '/study/d1' })
    await expectAccessible(container, () => heading(container, 'the flashcard review'))
  })
})
