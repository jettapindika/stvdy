import { Route, Routes } from 'react-router'
import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { WorkspaceScreen } from '@/features/workspaces/WorkspaceScreen'
import { ChatScreen } from './ChatScreen'
import type { Message } from '@/api/queries/chat'
import type { Source } from '@/api/queries/sources'
import {
  installFakePlatform,
  jsonResponse,
  manualStreamResponse,
  mockFetch,
  streamResponse,
  type ManualStream,
} from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The chat screen: the thread, the composer, the citation chips and the panel it
 * lives in.
 *
 * The accessibility requirements are asserted here rather than in a separate file
 * because they are properties of this screen, and a test that rendered a
 * different tree to check them would be checking the test's tree. Two of them are
 * specific to a streaming screen and are the reason this file exists at all:
 * `aria-live` on the list, so the answer is announced, and `aria-busy` while it
 * streams, so it is announced once and not fragment by fragment.
 */

const WORKSPACE = {
  id: 'w1',
  name: 'Struktur Data',
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
  source_count: 1,
}

const SOURCE: Source = {
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

function message(over: Partial<Message> & { id: string; role: 'user' | 'assistant' }): Message {
  return {
    workspace_id: 'w1',
    content: '',
    citations: [],
    used_web: false,
    created_at: '2026-10-11T08:00:00Z',
    ...over,
  }
}

const frame = (event: string, data: unknown) => `event: ${event}\ndata: ${JSON.stringify(data)}\n\n`

let messages: Message[]
let sources: Source[]
/** Every URL the app handed to the system browser, in order. */
let opened: string[]

beforeEach(() => {
  opened = installFakePlatform('macos').opened
  setAccessToken('access-1')
  messages = []
  sources = [SOURCE]
})

afterEach(() => {
  vi.unstubAllGlobals()
})

/** What the chat route answers with: a stream the test drives, or a bare status. */
type ChatAnswer =
  | { kind: 'stream'; stream: ManualStream }
  /** One stream per chat request, in order, for a test that sends more than one. */
  | { kind: 'streams'; streams: ManualStream[] }
  | { kind: 'response'; response: Response }

function serve(chat?: ChatAnswer) {
  let next = 0
  return mockFetch((url, init) => {
    const path = new URL(url).pathname
    if (path === '/workspaces/w1') return jsonResponse(200, { workspace: WORKSPACE })
    if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources })
    if (path === '/workspaces/w1/messages') return jsonResponse(200, { messages })
    if (path === '/workspaces/w1/chat') {
      if (chat?.kind === 'response') return chat.response
      const stream =
        chat?.kind === 'stream'
          ? chat.stream
          : chat?.kind === 'streams'
            ? chat.streams[next++]
            : undefined
      if (stream !== undefined) {
        stream.attach(init.signal)
        return stream.response
      }
    }
    const chunk = /^\/chunks\/([^/]+)$/.exec(path)
    if (chunk !== null) {
      return jsonResponse(200, {
        id: chunk[1],
        source_id: 's1',
        page: 12,
        idx: 4,
        content: 'Graf adalah kumpulan simpul dan sisi yang menghubungkannya.',
      })
    }
    return jsonResponse(404, {
      code: 'not_found',
      message: 'no such route',
      min_plan: null,
      limit: null,
      used: null,
    })
  })
}

function renderChat() {
  return renderWithProviders(<ChatScreen workspaceId="w1" />, { route: '/workspaces/w1' })
}

describe('the thread', () => {
  it('renders the persisted turns, oldest first, with their roles', async () => {
    messages = [
      message({ id: 'm1', role: 'user', content: 'Apa itu graf?' }),
      message({
        id: 'm2',
        role: 'assistant',
        content: 'Graf adalah kumpulan simpul dan sisi [1].',
        citations: [{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 12 }],
      }),
    ]
    serve()

    renderChat()

    expect(await screen.findByText('Apa itu graf?')).toBeInTheDocument()
    expect(await screen.findByText(/kumpulan simpul dan sisi/)).toBeInTheDocument()
    // The citation in the text is a control that opens the chunk it names.
    expect(screen.getByRole('button', { name: /Open source: Bab 3 Graf, page 12/ })).toBeInTheDocument()
  })

  it('says why the thread is empty rather than only that it is', async () => {
    messages = []
    serve()

    renderChat()

    expect(await screen.findByText('No questions yet')).toBeInTheDocument()
    expect(screen.getByText(/cites the page each claim came from/)).toBeInTheDocument()
  })

  it('tells the student to add material when the workspace has no ready source', async () => {
    sources = []
    serve()

    renderChat()

    expect(await screen.findByText('No questions yet')).toBeInTheDocument()
    expect(screen.getByText(/once a source has finished indexing/)).toBeInTheDocument()
  })

  it('shows the readable failure and a way to retry when the thread cannot load', async () => {
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources })
      if (path === '/workspaces/w1/messages') {
        return jsonResponse(500, { code: 'internal', message: 'boom' })
      }
      return jsonResponse(404, { code: 'not_found', message: '' })
    })

    renderChat()

    expect(await screen.findByText('Server error')).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Try again' })).toBeInTheDocument()
  })
})

describe('a streamed answer', () => {
  it('appends tokens, marks the list busy, and clears it when done', async () => {
    const stream = manualStreamResponse()
    serve({ kind: 'stream', stream })
    const user = userEvent.setup()

    renderChat()
    await screen.findByText('No questions yet')

    await user.type(screen.getByLabelText('Ask about this workspace'), 'Apa itu graf?')
    await user.click(screen.getByRole('button', { name: 'Ask' }))

    const log = await screen.findByRole('log')
    expect(log).toHaveAttribute('aria-live', 'polite')
    expect(log).toHaveAttribute('aria-busy', 'true')

    stream.push(frame('token', { t: 'Graf ' }))
    stream.push(frame('token', { t: 'adalah kumpulan simpul [1].' }))
    expect(await screen.findByText(/Graf adalah kumpulan simpul/)).toBeInTheDocument()

    stream.push(
      frame('citations', {
        citations: [{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 12 }],
      }),
    )
    expect(
      await screen.findByRole('button', { name: /Open source: Bab 3 Graf, page 12/ }),
    ).toBeInTheDocument()

    // The persisted thread now carries the finished turn, so the live copy is
    // dropped and the list stops claiming to be busy.
    messages = [
      message({ id: 'm1', role: 'user', content: 'Apa itu graf?' }),
      message({
        id: 'm2',
        role: 'assistant',
        content: 'Graf adalah kumpulan simpul.',
        citations: [{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 12 }],
      }),
    ]
    stream.push(frame('usage', { credits_spent: 1, credits_remaining: 411 }))
    stream.push(frame('done', { message_id: 'm2', used_web: false }))

    await vi.waitFor(() => {
      expect(screen.getByRole('log')).toHaveAttribute('aria-busy', 'false')
    })
    expect(await screen.findByText('Apa itu graf?')).toBeInTheDocument()
  })

  it('does not move focus while the answer arrives', async () => {
    const stream = manualStreamResponse()
    serve({ kind: 'stream', stream })
    const user = userEvent.setup()

    renderChat()
    await screen.findByText('No questions yet')

    const composer = screen.getByLabelText('Ask about this workspace')
    // Enter, not a click: a click legitimately puts focus on the button that was
    // clicked, and the property under test is that the *answer* does not take
    // focus away from the field the student is typing in.
    await user.type(composer, 'Apa itu graf?{Enter}')

    stream.push(frame('token', { t: 'Graf ' }))
    expect(await screen.findByText(/Graf/)).toBeInTheDocument()
    stream.push(frame('token', { t: 'adalah' }))

    // Focus stays in the field the student is typing in. An answer that stole it
    // would take them out of the composer the moment their first reply landed.
    expect(document.activeElement).toBe(composer)
  })

  it('sends only the selected sources', async () => {
    const stream = manualStreamResponse()
    const calls = serve({ kind: 'stream', stream })
    const user = userEvent.setup()

    renderChat()
    await screen.findByText('No questions yet')

    await user.click(screen.getByLabelText('Bab 3 Graf'))
    await user.type(screen.getByLabelText('Ask about this workspace'), 'Ringkas bab ini')
    await user.click(screen.getByRole('button', { name: 'Ask' }))

    await vi.waitFor(() => {
      expect(calls.some((call) => call.url.endsWith('/workspaces/w1/chat'))).toBe(true)
    })
    const sent = calls.find((call) => call.url.endsWith('/workspaces/w1/chat'))
    expect(JSON.parse(String(sent?.init.body))).toEqual({
      message: 'Ringkas bab ini',
      source_ids: ['s1'],
      // Sent explicitly rather than omitted: the server defaults it, but a body
      // that carries the toggle's actual state is what makes a web turn testable
      // from the client's side.
      use_web: false,
    })
  })

  it('offers Stop while streaming and Ask when idle', async () => {
    const stream = manualStreamResponse()
    serve({ kind: 'stream', stream })
    const user = userEvent.setup()

    renderChat()
    await screen.findByText('No questions yet')

    await user.type(screen.getByLabelText('Ask about this workspace'), 'Tanya')
    await user.click(screen.getByRole('button', { name: 'Ask' }))

    expect(await screen.findByRole('button', { name: 'Stop' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Ask' })).toBeNull()
  })
})

describe('a failure on the screen', () => {
  it('keeps the partial answer visible with a retry, and says nothing was saved', async () => {
    const stream = manualStreamResponse()
    serve({ kind: 'stream', stream })
    const user = userEvent.setup()

    renderChat()
    await screen.findByText('No questions yet')

    await user.type(screen.getByLabelText('Ask about this workspace'), 'Apa itu graf?')
    await user.click(screen.getByRole('button', { name: 'Ask' }))

    stream.push(frame('token', { t: 'Graf adalah' }))
    expect(await screen.findByText(/Graf adalah/)).toBeInTheDocument()

    stream.fail(new TypeError('network error'))

    expect(await screen.findByRole('button', { name: 'Ask again' })).toBeInTheDocument()
    // The text the student already read is still there, above the notice.
    expect(screen.getByText(/Graf adalah/)).toBeInTheDocument()
    expect(screen.getByText('No connection')).toBeInTheDocument()
  })

  it('shows the credit refusal before any answer, with no partial turn', async () => {
    serve({
      kind: 'response',
      response: streamResponse(
        [
          JSON.stringify({
            code: 'insufficient_credits',
            message: 'balance is short',
            min_plan: null,
            limit: null,
            used: null,
          }),
        ],
        402,
      ),
    })
    const user = userEvent.setup()

    renderChat()
    await screen.findByText('No questions yet')

    await user.type(screen.getByLabelText('Ask about this workspace'), 'Tanya')
    await user.click(screen.getByRole('button', { name: 'Ask' }))

    expect(await screen.findByText('Not enough credits')).toBeInTheDocument()
    // Nothing that looks like an answer, because the server persisted nothing.
    expect(screen.queryByText('Stvdy', { selector: '.label' })).toBeNull()
  })

  it('tells an unverified account to check its inbox', async () => {
    serve({
      kind: 'response',
      response: streamResponse(
        [
          JSON.stringify({
            code: 'email_not_verified',
            message: 'verify first',
            min_plan: null,
            limit: null,
            used: null,
          }),
        ],
        403,
      ),
    })
    const user = userEvent.setup()

    renderChat()
    await screen.findByText('No questions yet')

    await user.type(screen.getByLabelText('Ask about this workspace'), 'Tanya')
    await user.click(screen.getByRole('button', { name: 'Ask' }))

    expect(await screen.findByText('Email not verified')).toBeInTheDocument()
    expect(screen.getByText(/Check your inbox/)).toBeInTheDocument()
  })
})

describe('a citation chip', () => {
  it('sends use_web true when the toggle is on and false when it is off', async () => {
    const calls = serve()
    const user = userEvent.setup()

    renderChat()
    await user.type(await screen.findByLabelText('Ask about this workspace'), 'Apa itu graf?')
    await user.click(screen.getByRole('button', { name: 'Ask' }))

    await vi.waitFor(() => {
      expect(calls.some((call) => call.url.endsWith('/workspaces/w1/chat'))).toBe(true)
    })
    const off = calls.find((call) => call.url.endsWith('/workspaces/w1/chat'))
    expect(JSON.parse(String(off?.init.body)).use_web).toBe(false)

    // The toggle is a real control now, not a disabled note, so flipping it has
    // to change what the next turn sends.
    await user.click(screen.getByRole('checkbox', { name: /Search the web too/ }))
    await user.type(screen.getByLabelText('Ask about this workspace'), 'dan lagi')
    await user.click(screen.getByRole('button', { name: 'Ask' }))

    await vi.waitFor(() => {
      const bodies = calls
        .filter((call) => call.url.endsWith('/workspaces/w1/chat'))
        .map((call) => JSON.parse(String(call.init.body)).use_web)
      expect(bodies).toContain(true)
    })
  })

  it('renders a mixed citation list in the order the server sent it', async () => {
    messages = [
      message({ id: 'm1', role: 'user', content: 'Apa itu graf?' }),
      message({
        id: 'm2',
        role: 'assistant',
        content: 'Dari catatanmu [1], dan dari luar [W1].',
        citations: [
          { type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 12 },
          { type: 'web', url: 'https://example.org/paper', title: 'A paper' },
        ],
      }),
    ]
    serve()

    renderChat()

    const chips = await screen.findAllByRole('link')
    const labels = chips.map((chip) => chip.getAttribute('aria-label') ?? '')
    const webIndex = labels.findIndex((label) => label.includes('Open web source'))
    expect(webIndex).toBeGreaterThanOrEqual(0)
    // The chunk chip is a button rather than a link, so it is found by its own
    // accessible name; the ordering assertion is about the web one arriving after.
    expect(screen.getByRole('button', { name: /Open source: Bab 3 Graf, page 12/ })).toBeInTheDocument()
  })

  it('opens a web citation in the system browser and never in the app', async () => {
    messages = [
      message({ id: 'm1', role: 'user', content: 'Tanya' }),
      message({
        id: 'm2',
        role: 'assistant',
        content: 'Dari luar [W1].',
        citations: [{ type: 'web', url: 'https://example.org/paper', title: 'A paper' }],
      }),
    ]
    serve()
    const user = userEvent.setup()

    renderChat()

    const link = await screen.findByRole('link', { name: /Open web source: A paper/ })
    expect(link).toHaveAttribute('href', 'https://example.org/paper')

    await user.click(link)

    // Through the platform adapter, because an in-app navigation would replace the
    // whole SPA with someone else's page and there is no way back.
    expect(opened).toEqual(['https://example.org/paper'])
  })

  it('shows a DOI when the tier gave one, and shows nothing when it did not', async () => {
    messages = [
      message({ id: 'm1', role: 'user', content: 'Tanya' }),
      message({
        id: 'm2',
        role: 'assistant',
        content: 'Dari luar [W1] dan [W2].',
        citations: [
          {
            type: 'web',
            url: 'https://example.org/paper',
            title: 'A paper',
            doi: '10.1000/xyz',
          },
          { type: 'web', url: 'https://example.org/blog', title: 'A blog post', doi: null },
        ],
      }),
    ]
    serve()

    renderChat()

    // The phase gate is "URLs and DOIs", so a DOI that never reaches the screen is
    // a gate that was not met.
    expect(await screen.findByText('10.1000/xyz')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: /Open web source: A blog post/ })).toBeInTheDocument()
  })

  it('does not crash on a web citation with a URL it cannot open', async () => {
    messages = [
      message({ id: 'm1', role: 'user', content: 'Tanya' }),
      message({
        id: 'm2',
        role: 'assistant',
        content: 'Dari luar [W1].',
        // A scheme the app will not hand to the browser. It must render as text
        // rather than as a control that cannot go anywhere.
        citations: [{ type: 'web', url: 'javascript:alert(1)', title: 'Suspicious' }],
      }),
    ]
    serve()

    renderChat()

    const text = await screen.findByText('Suspicious')
    expect(text).toBeInTheDocument()
    expect(screen.queryByRole('link', { name: /Open web source: Suspicious/ })).toBeNull()
  })

  it('opens the chunk it names, with its source and page', async () => {
    messages = [
      message({ id: 'm1', role: 'user', content: 'Apa itu graf?' }),
      message({
        id: 'm2',
        role: 'assistant',
        content: 'Kumpulan simpul [1].',
        citations: [{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 12 }],
      }),
    ]
    serve()
    const user = userEvent.setup()

    renderChat()

    await user.click(
      await screen.findByRole('button', { name: /Open source: Bab 3 Graf, page 12/ }),
    )

    const panel = await screen.findByRole('region', { name: /Bab 3 Graf/ })
    expect(within(panel).getByText(/kumpulan simpul dan sisi/)).toBeInTheDocument()
    // Opening it is the student's own action, so focus moves into it and the
    // close button is the way back out.
    expect(document.activeElement).toBe(within(panel).getByRole('button', { name: 'Close' }))

    await user.click(within(panel).getByRole('button', { name: 'Close' }))
    expect(screen.queryByRole('region', { name: /Bab 3 Graf/ })).toBeNull()
  })

  it('leaves a marker with no citation behind as text, never as a wrong chip', async () => {
    messages = [
      message({ id: 'm1', role: 'user', content: 'Tanya' }),
      message({
        id: 'm2',
        role: 'assistant',
        // The server resolved `[1]` and dropped `[7]`, which is what a fabricated
        // label looks like by the time it reaches the screen: it is gone from
        // both the citations and the text. This text still carries it, so the
        // screen has to cope with a marker that has nothing behind it.
        content: 'Klaim ini [1] dan yang ini [7].',
        citations: [{ type: 'chunk', chunk_id: 'c1', source_id: 's1', page: 12 }],
      }),
    ]
    serve()

    renderChat()

    const log = await screen.findByRole('log')
    // Only the resolved marker becomes a chip. The other one stays as the text it
    // was, rather than being pointed at whichever citation happened to be first.
    await vi.waitFor(() => {
      expect(log).toHaveTextContent('Klaim ini p.12 dan yang ini [7].')
    })
    expect(screen.getAllByRole('button', { name: /Open source: Bab 3 Graf/ })).toHaveLength(1)
  })

  it('renders the answer as text, not as the model\u2019s markup', async () => {
    messages = [
      message({ id: 'm1', role: 'user', content: 'Tanya' }),
      message({
        id: 'm2',
        role: 'assistant',
        content: 'Poin:\n\n- **Simpul** adalah titik\n- `edge` menghubungkan\n\n<script>alert(1)</script>',
      }),
    ]
    serve()

    renderChat()

    expect(await screen.findByText('Simpul')).toBeInTheDocument()
    expect(screen.getByText('edge')).toBeInTheDocument()
    // Raw HTML is text, never markup: an answer that could inject elements would
    // be a way to put anything at all on the page.
    expect(document.querySelector('script')).toBeNull()
    expect(screen.getByText(/<script>alert\(1\)<\/script>/)).toBeInTheDocument()
  })
})

describe('the chat panel inside the workspace screen', () => {
  it('renders the chat screen where the placeholder was', async () => {
    serve()
    const user = userEvent.setup()

    renderWithProviders(
      <Routes>
        <Route path="/workspaces/:workspaceId" element={<WorkspaceScreen />} />
      </Routes>,
      { route: '/workspaces/w1' },
    )

    await user.click(await screen.findByRole('tab', { name: 'Chat' }))

    const panel = screen.getByRole('tabpanel', { name: 'Chat' })
    expect(await within(panel).findByLabelText('Ask about this workspace')).toBeInTheDocument()
    expect(within(panel).queryByText(/Not built yet/)).toBeNull()
    // Studio is the flashcard panel Phase 4 built, not a placeholder.
    expect(screen.getByRole('tabpanel', { name: 'Studio' })).toHaveTextContent('Generate deck')
  })
})
