import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { setTimeout as sleep } from 'node:timers/promises'
import { Route, Routes } from 'react-router'

import { CheatsheetScreen } from './CheatsheetScreen'
import type { Cheatsheet } from '@/api/queries/cheatsheets'
import { installFakePlatform, jsonResponse, mockFetch, type RecordedCall } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The three rules this screen exists to get right.
 *
 * **The generated document is untrusted input.** The model wrote the sheet's text
 * and the renderer assembled the page from typed blocks, but the result is still a
 * whole HTML document that this app puts in front of the student's own origin. The
 * test asserts the frame's `sandbox` attribute is present and empty, because an
 * empty sandbox is the property: no scripts, no forms, no top-level navigation, no
 * same-origin access. A `sandbox="allow-scripts"` added later for a "small fix"
 * would show up here.
 *
 * **The page count is the server's.** `page_count` is what the printed footer's
 * `Halaman N dari M` depends on, so the screen renders the number it was given and
 * shows nothing when there is none. A count derived from the depth here would be a
 * different number from the one on the page, which is exactly the invented metric
 * the honesty gate forbids.
 *
 * **PDF is plan-gated by `export_formats`.** Not by a conditional in this file: the
 * matrix decides, and the plan that does not include `pdf` gets no download action
 * at all. On a phone build the locked variant carries no price and no way to buy.
 */

const SHEET: Cheatsheet = {
  id: 'c1',
  workspace_id: 'w1',
  title: 'Jaringan Komputer',
  scope: 'UTS Jaringan Komputer',
  exam_date: '2026-10-20',
  depth: 'normal',
  source_ids: ['s1'],
  status: 'ready',
  error: null,
  template_version: 'v1',
  page_count: 6,
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
}

const HTML = '<!doctype html><html><body><h1>Jaringan Komputer</h1></body></html>'

function user(planId: string) {
  return {
    id: 'u1',
    email: 'student@example.com',
    email_verified: true,
    email_verified_at: '2026-10-11T08:00:00Z',
    plan_id: planId,
    timezone: 'Asia/Jakarta',
    student_verified: false,
    created_at: '2026-10-11T08:00:00Z',
  }
}

/** A response whose body is the document itself, which is what these routes answer. */
function textResponse(body: string, type: string): Response {
  return {
    ok: true,
    status: 200,
    text: async () => body,
    blob: async () => new Blob([body], { type }),
  } as Response
}

beforeEach(() => {
  installFakePlatform('macos')
  setAccessToken('access-1')
})

afterEach(() => {
  vi.unstubAllGlobals()
})

interface Served {
  calls: RecordedCall[]
  rerendered: number
}

function serve(options: { plan?: string; sheet?: Cheatsheet } = {}): Served {
  let rerendered = 0
  const calls = mockFetch((url, init) => {
    const path = new URL(url).pathname
    const method = init.method ?? 'GET'
    if (path === '/auth/me') return jsonResponse(200, user(options.plan ?? 'pro'))
    if (path === '/cheatsheets/c1' && method === 'GET') {
      return jsonResponse(200, { cheatsheet: options.sheet ?? SHEET })
    }
    if (path === '/cheatsheets/c1/html') return textResponse(HTML, 'text/html')
    if (path === '/cheatsheets/c1/pdf') return textResponse('%PDF-1.4', 'application/pdf')
    if (path === '/cheatsheets/c1/re-render' && method === 'POST') {
      rerendered += 1
      return jsonResponse(200, { cheatsheet: options.sheet ?? SHEET })
    }
    return jsonResponse(404, {
      code: 'not_found',
      message: 'no such route',
      min_plan: null,
      limit: null,
      used: null,
    })
  })
  return {
    calls,
    get rerendered() {
      return rerendered
    },
  }
}

function renderSheet() {
  return renderWithProviders(
    <Routes>
      <Route path="/cheatsheets/:cheatsheetId" element={<CheatsheetScreen />} />
    </Routes>,
    { route: '/cheatsheets/c1' },
  )
}

describe('the preview frame', () => {
  it('renders the generated document in an empty sandbox', async () => {
    serve()
    renderSheet()

    const frame = await screen.findByTitle('Preview')
    // `sandbox` with no value: a unique opaque origin, no scripts, no forms, no
    // top-level navigation. Any token added here would be a capability the
    // generated document does not need.
    expect(frame.getAttribute('sandbox')).toBe('')
    // And the frame is named: a document a screen reader user cannot identify is a
    // landmark with no label. axe cannot check this one in jsdom (see
    // `features/a11y.test.tsx`), so it is asserted here.
    expect(frame).toHaveAccessibleName('Preview')
    // The document is inlined rather than linked: the route needs the bearer
    // token, which a frame navigation cannot carry.
    expect(frame.getAttribute('srcdoc')).toBe(HTML)
    expect(frame.getAttribute('src')).toBeNull()
  })

  it('shows the page count the renderer reported', async () => {
    serve()
    renderSheet()

    // Six, as the renderer counted it for the print — the same number the
    // footer's `Halaman N dari M` is printed from.
    expect(await screen.findByText('6 pages')).toBeInTheDocument()
  })

  it('names the sheet by its scope alone when there is no exam date', async () => {
    // The date is omitted rather than paired with an empty half: the header carries
    // the scope, and a sheet with no exam on the calendar says only what it is for.
    // Scoped to the header because the footer repeats the scope.
    serve({ sheet: { ...SHEET, exam_date: null } })
    renderSheet()

    const header = await screen.findByRole('banner')
    expect(within(header).getByText('UTS Jaringan Komputer')).toBeInTheDocument()
    expect(within(header).queryByText(/·/)).not.toBeInTheDocument()
  })

  it('shows no page count at all when the renderer has not reported one', async () => {
    // No PDF yet, so no count. Not an estimate from the depth and not a zero: the
    // screen says nothing rather than something it cannot know.
    serve({ sheet: { ...SHEET, page_count: null } })
    renderSheet()

    expect(await screen.findByTitle('Preview')).toBeInTheDocument()
    expect(screen.queryByText(/^\d+ pages?$/)).not.toBeInTheDocument()
    expect(screen.queryByText('The PDF is null pages long.')).not.toBeInTheDocument()
  })

  /**
   * The frame is a `Tab` stop — it is a scrollable region, and a keyboard user
   * needs to land in it to scroll the document — but a focused `<iframe>` matches
   * no focus pseudo-class, so the global `:focus-visible` ring in `styles.css`
   * cannot reach it. Measured in Chromium: `document.activeElement` is the frame,
   * and `frame.matches(':focus')`, `:focus-visible` and `:focus-within` are all
   * false, so the stop had no visible ring at all.
   *
   * The ring is therefore driven by the one signal the browser does give, the
   * window's own blur/focus pair, and this test pins that mechanism: without it
   * (or with the `hasFocus()` guard dropped) the frame is a silent focus stop
   * again, which is invisible in every other test on this screen.
   */
  it('rings the frame when focus is inside it, and not when the window merely lost focus', async () => {
    serve()
    renderSheet()

    const frame = await screen.findByTitle('Preview')
    expect(frame.className).not.toContain('outline-ring')

    // Focus entering the frame blurs the window, with the frame as the document's
    // active element and the document still focused. That is the pair the effect
    // keys on.
    frame.focus()
    expect(document.activeElement).toBe(frame)
    window.dispatchEvent(new Event('blur'))
    expect((await screen.findByTitle('Preview')).className).toContain('outline-ring')

    // Focus leaving the frame returns the window's focus, and the ring goes.
    window.dispatchEvent(new Event('focus'))
    expect((await screen.findByTitle('Preview')).className).not.toContain('outline-ring')

    // An app switch also blurs the window, but the document is not focused then,
    // so a ring must not be drawn on a frame nobody is using.
    const hasFocus = vi.spyOn(document, 'hasFocus').mockReturnValue(false)
    window.dispatchEvent(new Event('blur'))
    expect((await screen.findByTitle('Preview')).className).not.toContain('outline-ring')
    hasFocus.mockRestore()
  })
})

describe('the PDF action, as the plan matrix allows it', () => {
  it('is absent on a plan whose formats are Markdown only', async () => {
    serve({ plan: 'free' })
    renderSheet()

    await screen.findByTitle('Preview')
    expect(screen.queryByRole('button', { name: 'Download PDF' })).not.toBeInTheDocument()
    // The reason stands where the action would have been.
    expect(screen.getByText('PDF export')).toBeInTheDocument()
    expect(screen.getByText('Available on Pro')).toBeInTheDocument()
  })

  it('is offered on a plan that includes the format, and fetches it', async () => {
    const served = serve({ plan: 'pro' })
    const handed: Blob[] = []
    vi.stubGlobal(
      'URL',
      Object.assign(URL, {
        createObjectURL: vi.fn((blob: Blob) => {
          handed.push(blob)
          return 'blob:pdf'
        }),
        revokeObjectURL: vi.fn(),
      }),
    )
    const user = userEvent.setup()
    renderSheet()

    await user.click(await screen.findByRole('button', { name: 'Download PDF' }))

    await vi.waitFor(() => {
      expect(served.calls.some((call) => call.url.endsWith('/cheatsheets/c1/pdf'))).toBe(true)
    })
    expect(handed).toHaveLength(1)
    expect(handed[0]?.type).toBe('application/pdf')
  })

  it('tells a phone build the fact and nothing that could be bought', async () => {
    installFakePlatform('android')
    serve({ plan: 'free' })
    const { container } = renderSheet()

    expect(await screen.findByText('Not available on your plan')).toBeInTheDocument()
    expect(screen.queryByText(/Available on/)).not.toBeInTheDocument()
    expect(container.querySelector('a[href="/billing"]')).toBeNull()
    // And no price anywhere: a credit figure on a phone build is the thing the
    // policy exists to keep out, and the free plan's own numbers are the leak that
    // would be easiest to miss.
    expect(screen.queryByText(/\d+ credits/)).not.toBeInTheDocument()
  })
})

describe('re-rendering', () => {
  it('calls the re-render route and says the call is free', async () => {
    const served = serve()
    const user = userEvent.setup()
    renderSheet()

    // Free is the fact the endpoint exists for: it re-runs the renderer over the
    // stored blocks, so it costs no credits and cannot change the content.
    expect(
      await screen.findByText('Re-rendering uses the stored content: no model call, and no credits.'),
    ).toBeInTheDocument()

    await user.click(screen.getByRole('button', { name: 'Re-render' }))
    await vi.waitFor(() => {
      expect(served.rerendered).toBe(1)
    })
  })
})

describe('a cheatsheet that cannot be shown', () => {
  it('polls a pending sheet until it is ready, then stops', async () => {
    // Real time on purpose, the same reason the workspace's source test uses it:
    // `refetchInterval` is a timer the query client owns, and a faked clock would
    // prove the timer fires without proving it stops. The bug this catches is the
    // one that was there — a fresh cheatsheet is `pending`, not `generating`, so a
    // poll condition that only looked for `generating` left the screen saying
    // "still being generated" for as long as the tab stayed open.
    let calls = 0
    mockFetch((url) => {
      const path = new URL(url).pathname
      if (path === '/auth/me') return jsonResponse(200, user('pro'))
      if (path === '/cheatsheets/c1') {
        calls += 1
        return jsonResponse(200, {
          cheatsheet: {
            ...SHEET,
            status: calls === 1 ? 'pending' : 'ready',
            page_count: calls === 1 ? null : 6,
          },
        })
      }
      if (path === '/cheatsheets/c1/html') return textResponse(HTML, 'text/html')
      return jsonResponse(404, {
        code: 'not_found',
        message: 'no such route',
        min_plan: null,
        limit: null,
        used: null,
      })
    })
    renderSheet()

    expect(await screen.findByText('This cheatsheet is still being generated')).toBeInTheDocument()

    await vi.waitFor(
      () => {
        expect(screen.queryByText('This cheatsheet is still being generated')).not.toBeInTheDocument()
      },
      { timeout: 5000 },
    )
    expect(await screen.findByText('6 pages')).toBeInTheDocument()

    // And then it stops: a settled document is not asked about again.
    const settled = calls
    await sleep(3200)
    expect(calls).toBe(settled)
  }, 20000)

  it('shows the failure with the reason the server gave', async () => {
    serve({
      sheet: {
        ...SHEET,
        status: 'failed',
        error: 'The blocks did not validate against the template contract',
        page_count: null,
        template_version: null,
      },
    })
    renderSheet()

    expect(await screen.findByText('This cheatsheet could not be generated')).toBeInTheDocument()
    expect(
      screen.getByText('The blocks did not validate against the template contract'),
    ).toBeInTheDocument()
  })

  it('says the document is still being generated rather than showing an empty preview', async () => {
    serve({ sheet: { ...SHEET, status: 'generating', page_count: null } })
    renderSheet()

    expect(await screen.findByText('This cheatsheet is still being generated')).toBeInTheDocument()
    expect(screen.queryByTitle('Preview')).not.toBeInTheDocument()
  })
})
