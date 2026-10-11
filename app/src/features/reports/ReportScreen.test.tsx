import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'
import { Route, Routes } from 'react-router'

import { ReportScreen } from './ReportScreen'
import type { Report } from '@/api/queries/reports'
import { installFakePlatform, jsonResponse, mockFetch, type RecordedCall } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The four rules this screen exists to get right, and none of them is layout.
 *
 * **The section index is the student's.** Generation is per section, by index, and
 * that is what makes regenerating one section cheap instead of paying for the
 * whole report again. The test asserts the index that actually went out, because
 * a button that always sent `0` would look identical on screen.
 *
 * **A gap is stated in words and marked with an icon.** The spec's `gaps: true`
 * means the sources do not support that section, and a student revising from the
 * outline has to see it before spending credits on it. It is text plus an icon,
 * never a colour: a colour alone is invisible in monochrome and silent to a screen
 * reader, and this is the one caveat on the screen that costs money to ignore.
 *
 * **The cost is on screen before the action, and it traces to `config/plans.toml`.**
 * The test reads the file from disk and asserts the numbers the screen renders are
 * the file's, so a literal that happened to match today cannot pass.
 *
 * **An export format the plan does not include is not offered.** The matrix decides,
 * and on a phone the locked variant must not carry a price or a way to buy.
 */

const PLAN_COSTS = (() => {
  const toml = readFileSync(join(__dirname, '..', '..', '..', '..', 'config', 'plans.toml'), 'utf8')
  const number = (key: string) =>
    Number(new RegExp(`^${key}\\s*=\\s*(\\d+)$`, 'm').exec(toml)?.[1] ?? '0')
  return { outline: number('report_outline'), section: number('report_section') }
})()

const OUTLINE = {
  title: 'Jaringan Komputer',
  sections: [
    {
      heading: 'Pendahuluan',
      purpose: 'Menetapkan ruang lingkup laporan.',
      labels: [1, 2],
      gaps: false,
    },
    {
      heading: 'Keamanan nirkabel',
      purpose: 'Menjelaskan mekanisme WPA3.',
      labels: [],
      gaps: true,
    },
  ],
}

function report(over: Partial<Report> = {}): Report {
  return {
    id: 'r1',
    workspace_id: 'w1',
    title: 'Jaringan Komputer',
    status: 'draft',
    error: null,
    outline: OUTLINE,
    body_md: null,
    sections: [
      { heading: 'Pendahuluan', body_md: null, generating: false, error: null },
      { heading: 'Keamanan nirkabel', body_md: null, generating: false, error: null },
    ],
    created_at: '2026-10-11T08:00:00Z',
    updated_at: '2026-10-11T08:00:00Z',
    ...over,
  }
}

/** The user `GET /auth/me` answers, whose plan decides which formats exist. */
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

/** A response whose body is a document, for the export download. */
function documentResponse(body: string, type: string): Response {
  return {
    ok: true,
    status: 200,
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
  generated: unknown[]
  exports: string[]
  calls: RecordedCall[]
}

function serve(
  options: { plan?: string; report?: Report; onGenerate?: () => Report } = {},
): Served {
  const generated: unknown[] = []
  const exports: string[] = []
  const calls = mockFetch((url, init) => {
    const path = new URL(url).pathname
    const method = init.method ?? 'GET'
    if (path === '/auth/me') return jsonResponse(200, user(options.plan ?? 'pro'))
    if (path === '/reports/r1' && method === 'GET') {
      return jsonResponse(200, { report: options.report ?? report() })
    }
    if (path === '/reports/r1/generate' && method === 'POST') {
      generated.push(JSON.parse(String(init.body)) as unknown)
      return jsonResponse(200, { report: options.onGenerate?.() ?? report() })
    }
    if (path === '/reports/r1' && method === 'PATCH') {
      const body = JSON.parse(String(init.body)) as { outline: typeof OUTLINE }
      return jsonResponse(200, { report: report({ outline: body.outline }) })
    }
    if (path === '/reports/r1/export') {
      exports.push(new URL(url).searchParams.get('format') ?? '')
      return documentResponse('# Jaringan Komputer\n', 'text/markdown')
    }
    return jsonResponse(404, {
      code: 'not_found',
      message: 'no such route',
      min_plan: null,
      limit: null,
      used: null,
    })
  })
  return { generated, exports, calls }
}

function renderReport() {
  return renderWithProviders(
    <Routes>
      <Route path="/reports/:reportId" element={<ReportScreen />} />
    </Routes>,
    { route: '/reports/r1' },
  )
}

describe('the editable outline', () => {
  it('states a gap in words and marks it with an icon, not a colour', async () => {
    serve()
    renderReport()

    // The heading is on screen with its caveat beside it.
    const warning = await screen.findByText("This isn't covered in your selected sources.")
    expect(warning).toBeInTheDocument()
    // The icon is the other half of the rule: the caveat is never carried by a
    // tone alone, and the tone is not what a screen reader or a monochrome print
    // gets. The icon sits beside the sentence, in the same caveat block.
    expect(warning.parentElement?.querySelector('svg')).not.toBeNull()
    // And only the section the model marked as a gap carries it: a caveat shown on
    // every section would be a caveat nobody reads.
    expect(
      screen.getAllByText("This isn't covered in your selected sources."),
    ).toHaveLength(1)
  })

  it('saves a renamed, reordered outline as the whole outline', async () => {
    const served = serve()
    const user = userEvent.setup()
    renderReport()

    const heading = await screen.findByLabelText('Section 1')
    await user.clear(heading)
    await user.type(heading, 'Latar belakang')
    await user.click(screen.getAllByRole('button', { name: 'Move down' })[0] as HTMLElement)
    await user.click(screen.getByRole('button', { name: 'Save outline' }))

    const patch = served.calls.find((call) => call.init.method === 'PATCH')
    expect(patch).toBeDefined()
    const body = JSON.parse(String(patch?.init.body)) as { outline: typeof OUTLINE }
    // The rename and the move are one write, and the moved section is now first.
    expect(body.outline.sections.map((section) => section.heading)).toEqual([
      'Keamanan nirkabel',
      'Latar belakang',
    ])
    expect(body.outline.sections[1]?.gaps).toBe(false)
  })
})

describe('generating a section', () => {
  it('sends the index of the section whose button was pressed', async () => {
    const calls = serve()
    const user = userEvent.setup()
    renderReport()

    // The second section, so a hardcoded `0` cannot pass.
    const buttons = await screen.findAllByRole('button', { name: 'Generate' })
    expect(buttons).toHaveLength(2)
    await user.click(buttons[1] as HTMLElement)

    expect(calls.generated).toEqual([{ index: 1 }])
  })

  it('shows the price of a section before the button that spends it', async () => {
    const calls = serve()
    renderReport()

    // The price is beside every section's button, while nothing has been
    // generated. The number is read from the config file in the test as well, so
    // the screen's number is the file's rather than a literal that happens to
    // match today.
    expect(PLAN_COSTS.section).toBeGreaterThan(0)
    const prices = await screen.findAllByText(`${String(PLAN_COSTS.section)} credits`)
    expect(prices).toHaveLength(2)
    // Each one is inside the row that carries the button it belongs to, so the
    // price is attached to the action rather than floating at the top of a page.
    const row = prices[0]?.closest('li')
    expect(row).not.toBeNull()
    expect(within(row as HTMLElement).getByRole('button', { name: 'Generate' })).toBeInTheDocument()
    // And nothing was spent by looking at it.
    expect(calls.generated).toHaveLength(0)
  })

  it('shows a written section as written, and offers to replace it', async () => {
    const written = report({
      status: 'ready',
      sections: [
        {
          heading: 'Pendahuluan',
          body_md: 'Jaringan komputer menghubungkan simpul [1].',
          generating: false,
          error: null,
        },
        { heading: 'Keamanan nirkabel', body_md: null, generating: false, error: null },
      ],
    })
    serve({ report: written })
    renderReport()

    expect(await screen.findByText('Jaringan komputer menghubungkan simpul [1].')).toBeInTheDocument()
    // One written, one not: the row that has text is offered a replacement and the
    // row that has none is offered a first generation.
    expect(screen.getAllByRole('button', { name: 'Regenerate' })).toHaveLength(1)
    expect(screen.getAllByRole('button', { name: 'Generate' })).toHaveLength(1)
    expect(screen.getByText('1 of 2 sections written')).toBeInTheDocument()
  })

  it('shows the progress the server reports, so a job survives a reload', async () => {
    // The section is being written by a job that outlives this tab, and the server
    // is what says so. A screen that read progress from its own pending mutation
    // would show "Not written yet" for a section that is already being written —
    // the state a reload, or the second device the product is premised on, lands in.
    serve({
      report: report({
        status: 'generating',
        sections: [
          { heading: 'Pendahuluan', body_md: null, generating: true, error: null },
          { heading: 'Keamanan nirkabel', body_md: null, generating: false, error: null },
        ],
      }),
    })
    renderReport()

    await screen.findAllByRole('listitem')
    // Scoped to the sections list: the outline above it is a list of rows too, and
    // the two are told apart by the heading they sit under.
    const section = screen.getByRole('heading', { name: 'Sections' }).closest('section')
    expect(section).not.toBeNull()
    const rows = within(section as HTMLElement).getAllByRole('listitem')
    // The in-flight section says so, in the chip and on its own button; the other
    // one is still open and offered a first generation.
    expect(within(rows[0] as HTMLElement).getAllByText('Writing')).toHaveLength(2)
    expect(within(rows[0] as HTMLElement).getByRole('button', { name: 'Writing' })).toBeDisabled()
    expect(within(rows[1] as HTMLElement).getByText('Not written yet')).toBeInTheDocument()
    expect(within(rows[1] as HTMLElement).getByRole('button', { name: 'Generate' })).toBeEnabled()
  })
})

describe('the export, as the plan matrix allows it', () => {
  it('offers no PDF action on a plan whose formats are Markdown only', async () => {
    serve({ plan: 'free' })
    renderReport()

    // Free's `export_formats` is `["md"]`, so the PDF button does not exist: not
    // disabled, not hidden — absent, which is what a phone build must ship.
    expect(await screen.findByRole('button', { name: 'Download Markdown' })).toBeInTheDocument()
    expect(screen.queryByRole('button', { name: 'Download PDF' })).not.toBeInTheDocument()
    // And the reason is stated where the action would have been, naming the plan.
    expect(screen.getByText('PDF export')).toBeInTheDocument()
    expect(screen.getByText('Available on Pro')).toBeInTheDocument()
  })

  it('offers both formats on a plan that includes them', async () => {
    serve({ plan: 'pro' })
    renderReport()

    expect(await screen.findByRole('button', { name: 'Download Markdown' })).toBeInTheDocument()
    expect(screen.getByRole('button', { name: 'Download PDF' })).toBeInTheDocument()
    expect(screen.queryByText('PDF export')).not.toBeInTheDocument()
  })

  it('tells a phone build the fact and nothing that could be bought', async () => {
    installFakePlatform('ios')
    serve({ plan: 'free' })
    const { container } = renderReport()

    expect(await screen.findByText('Not available on your plan')).toBeInTheDocument()
    // No price, no upgrade link, no route to checkout: the stores review for this,
    // and "the server would have refused" is not a defence.
    expect(screen.queryByText(/Available on/)).not.toBeInTheDocument()
    expect(container.querySelector('a[href="/billing"]')).toBeNull()
  })

  it('downloads the format that was asked for', async () => {
    const calls = serve({ plan: 'free' })
    // Typed with the argument it is called with, so the assertion can read what
    // the platform was handed rather than only that it was called.
    const handed: Blob[] = []
    vi.stubGlobal(
      'URL',
      Object.assign(URL, {
        createObjectURL: vi.fn((blob: Blob) => {
          handed.push(blob)
          return 'blob:report'
        }),
        revokeObjectURL: vi.fn(),
      }),
    )
    const user = userEvent.setup()
    renderReport()

    await user.click(await screen.findByRole('button', { name: 'Download Markdown' }))

    await vi.waitFor(() => {
      expect(calls.exports).toEqual(['md'])
    })
    // The bytes that were fetched are the bytes that were saved, with the type the
    // export route served.
    expect(handed).toHaveLength(1)
    expect(handed[0]?.type).toBe('text/markdown')
  })
})

describe('a report that cannot be shown', () => {
  it('shows the failure with the reason the server gave', async () => {
    serve({
      report: report({
        status: 'failed',
        error: 'The model returned a section that did not parse',
        outline: null,
        sections: [],
      }),
    })
    renderReport()

    expect(await screen.findByText('This report could not be generated')).toBeInTheDocument()
    expect(
      screen.getByText('The model returned a section that did not parse'),
    ).toBeInTheDocument()
  })
})

describe('the outline rows', () => {
  it('names each row by its position so a screen reader can tell them apart', async () => {
    serve()
    renderReport()

    const fields = await screen.findAllByLabelText(/^Section \d$/)
    expect(fields.map((field) => field.getAttribute('aria-label') ?? field.id)).toHaveLength(2)
    // The purpose the model wrote is shown beside the heading it belongs to.
    const rows = screen.getAllByRole('listitem')
    expect(within(rows[0] as HTMLElement).getByText('Menetapkan ruang lingkup laporan.')).toBeInTheDocument()
  })
})
