import { readFileSync } from 'node:fs'
import { join } from 'node:path'

import { screen, within } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, beforeEach, describe, expect, it, vi } from 'vitest'

import { ReportPanel } from './ReportPanel'
import type { Report } from '@/api/queries/reports'
import { installFakePlatform, jsonResponse, mockFetch, type RecordedCall } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import { setAccessToken } from '@/lib/tokens'

/**
 * The one rule this panel must not break, and the one number it must not invent.
 *
 * **The prices come from `config/plans.toml`.** The honesty gate is that the cost
 * is visible *before* each generative action, and the file the server charges from
 * is the only honest source for it. The test reads the file itself and asserts the
 * numbers the panel renders are the file's, so a literal that happened to match
 * today cannot pass and a config change cannot leave the panel quoting a stale
 * price. It also asserts both prices are on screen before anything is pressed: the
 * outline's total *and* the per-section price that follows it.
 *
 * The panel is also where the cheatsheet is created, and the same rule applies to
 * it: a cheatsheet is priced as `report_section`, and that number is on screen
 * before the button that spends it.
 */

const PLAN_COSTS = (() => {
  const toml = readFileSync(join(__dirname, '..', '..', '..', '..', 'config', 'plans.toml'), 'utf8')
  const number = (key: string) =>
    Number(new RegExp(`^${key}\\s*=\\s*(\\d+)$`, 'm').exec(toml)?.[1] ?? '0')
  return { outline: number('report_outline'), section: number('report_section') }
})()

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

const REPORT: Report = {
  id: 'r1',
  workspace_id: 'w1',
  title: 'Jaringan Komputer',
  status: 'draft',
  error: null,
  outline: {
    title: 'Jaringan Komputer',
    sections: [
      { heading: 'Pendahuluan', purpose: 'Menetapkan ruang lingkup.', labels: [1], gaps: false },
      { heading: 'Penutup', purpose: 'Menyimpulkan.', labels: [2], gaps: false },
    ],
  },
  body_md: null,
  sections: [
    { heading: 'Pendahuluan', body_md: 'Teks [1].', generating: false, error: null },
    { heading: 'Penutup', body_md: null, generating: false, error: null },
  ],
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
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
  bodies: unknown[]
}

function serve(reports: Report[] = []): Served {
  const bodies: unknown[] = []
  const calls = mockFetch((url, init) => {
    const path = new URL(url).pathname
    const method = init.method ?? 'GET'
    if (path === '/workspaces/w1/reports' && method === 'GET') {
      return jsonResponse(200, { reports })
    }
    if (path === '/workspaces/w1/sources') return jsonResponse(200, { sources: [SOURCE] })
    if (path === '/workspaces/w1/reports/outline' && method === 'POST') {
      bodies.push(JSON.parse(String(init.body)) as unknown)
      return jsonResponse(201, { report: REPORT })
    }
    if (path === '/workspaces/w1/cheatsheets' && method === 'POST') {
      bodies.push(JSON.parse(String(init.body)) as unknown)
      return jsonResponse(201, {
        cheatsheet: {
          id: 'c1',
          workspace_id: 'w1',
          title: 'Jaringan Komputer',
          scope: 'UTS',
          exam_date: null,
          depth: 'normal',
          source_ids: ['s1'],
          status: 'pending',
          error: null,
          template_version: null,
          page_count: null,
          created_at: '2026-10-11T08:00:00Z',
          updated_at: '2026-10-11T08:00:00Z',
        },
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
  return { calls, bodies }
}

function renderPanel() {
  return renderWithProviders(<ReportPanel workspaceId="w1" />, { route: '/workspaces/w1' })
}

/**
 * The form a labelled field belongs to.
 *
 * Both forms on this panel render a `SourcePicker` over the same sources, so an
 * unscoped `getByRole('checkbox', { name: 'Bab 3 Graf' })` finds two real
 * checkboxes. Scoping to the form is what the student does too: the outline's
 * sources are not the cheatsheet's.
 */
function formWith(label: string): HTMLElement {
  const field = screen.getByLabelText(label)
  const form = field.closest('form')
  if (form === null) throw new Error(`${label} is not inside a form`)
  return form
}

describe('the credit prices the panel quotes', () => {
  it('shows both prices before either action, from the plan matrix', async () => {
    const served = serve()
    renderPanel()

    // Both numbers, and both from the file rather than from a literal. The
    // outline's price is what the button starts; the section price is what the
    // student is really buying, and a number that only appeared after the first
    // purchase would not be a cost shown before the action.
    expect(PLAN_COSTS.outline).toBeGreaterThan(0)
    expect(PLAN_COSTS.section).toBeGreaterThan(0)
    expect(
      await screen.findByText(
        `Costs ${String(PLAN_COSTS.outline)} credits for the outline, then ${String(PLAN_COSTS.section)} credits for each section you generate.`,
      ),
    ).toBeInTheDocument()
    expect(
      screen.getByText(`Costs ${String(PLAN_COSTS.section)} credits. The cheatsheet appears here while it is generated.`),
    ).toBeInTheDocument()
    // Nothing was spent by looking at the form.
    expect(served.bodies).toHaveLength(0)
  })
})

describe('proposing an outline', () => {
  it('sends the topic, the style, the length and the sources', async () => {
    const served = serve()
    const user = userEvent.setup()
    renderPanel()

    await user.type(await screen.findByLabelText('Report topic'), 'Jaringan Komputer')
    await user.type(screen.getByLabelText('Style'), 'exam revision, formal')
    await user.type(screen.getByLabelText('Length'), 'about 1500 words')
    const form = formWith('Report topic')
    await user.click(within(form).getByRole('checkbox', { name: /Bab 3 Graf/ }))
    await user.click(within(form).getByRole('button', { name: 'Propose outline' }))

    expect(served.bodies).toEqual([
      {
        topic: 'Jaringan Komputer',
        style: 'exam revision, formal',
        length: 'about 1500 words',
        source_ids: ['s1'],
      },
    ])
  })

  it('will not propose an outline with no topic or no source', async () => {
    serve()
    const user = userEvent.setup()
    renderPanel()

    await screen.findByLabelText('Report topic')
    const form = formWith('Report topic')
    const submit = within(form).getByRole('button', { name: 'Propose outline' })
    expect(submit).toBeDisabled()

    // A topic alone is not enough: an outline with no sources to draw on is the
    // gap case for every section, which is not what this button is for.
    await user.type(screen.getByLabelText('Report topic'), 'Jaringan')
    await user.type(screen.getByLabelText('Style'), 'formal')
    await user.type(screen.getByLabelText('Length'), 'short')
    expect(within(form).getByRole('button', { name: 'Propose outline' })).toBeDisabled()

    await user.click(within(form).getByRole('checkbox', { name: /Bab 3 Graf/ }))
    expect(within(form).getByRole('button', { name: 'Propose outline' })).toBeEnabled()
  })
})

describe('the reports list', () => {
  it('shows how many sections have been written and how many are left', async () => {
    serve([REPORT])
    renderPanel()

    // One of two, counted from the outline and the stored bodies the server sent
    // rather than from a number the report carries.
    expect(await screen.findByText('1 of 2 written')).toBeInTheDocument()
    expect(screen.getByText('Outline only')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Open report' })).toHaveAttribute(
      'href',
      '/reports/r1',
    )
  })

  it('names the empty state and what fills it', async () => {
    serve([])
    renderPanel()

    expect(await screen.findByText('No reports yet')).toBeInTheDocument()
    expect(screen.getByText(/propose an outline/)).toBeInTheDocument()
  })

  /**
   * A row whose outline has no `sections` must not take the panel down.
   *
   * The server passes the stored `outline` through verbatim, so a hand-written or
   * legacy row can hold `{"title":"…"}` with no `sections` at all. This row's
   * count read `report.outline?.sections.length` and threw `Cannot read properties
   * of undefined (reading 'length')` — measured in Chromium against the real API,
   * where it replaced the whole Studio panel with React Router's error boundary,
   * which is what made the report screen (and so the PDF export) unreachable from
   * the UI at all. The row still carries a title, a status and a link, so it is
   * rendered with the count it can compute: no sections is not a count of zero
   * sections to claim, so the "written" chip is absent rather than saying "0 of 0".
   */
  it('renders a row whose stored outline has no sections list', async () => {
    serve([
      {
        ...REPORT,
        outline: { title: 'Laporan Uji' } as unknown as Report['outline'],
        sections: [],
      },
    ])
    renderPanel()

    expect(await screen.findByText('Jaringan Komputer')).toBeInTheDocument()
    expect(screen.getByText('Outline only')).toBeInTheDocument()
    expect(screen.getByRole('link', { name: 'Open report' })).toHaveAttribute(
      'href',
      '/reports/r1',
    )
    expect(screen.queryByText(/^\d+ of \d+ written$/)).not.toBeInTheDocument()
  })

  /**
   * The row's title must claim the whole line when the panel is narrow, and this is
   * the one place jsdom can see it.
   *
   * The three siblings on that row — a written count, a status chip and an "Open
   * report" button — are all effectively fixed-width, so a `flex-1` title has a
   * flex basis of `0` and is left whatever the three do not take. Measured in
   * Chromium at 390px before this fix: the report's own name rendered **16px wide
   * over 18 lines** (two characters per line), and 100px over 6 lines at 200% text
   * scale. `w-full` pushes the other three onto the next line at narrow widths and
   * `@md:flex-1` restores the single row when there is room for all four. Dropping
   * either class brings the squeeze back, which is why both are asserted rather than
   * the rendered text.
   *
   * **The breakpoint is a container one, not the viewport `sm:` this used to carry.**
   * The width that decides the row is the *panel's*: at 1365px the workspace grid
   * leaves this panel ~155px of content, so `sm:` fired on the desktop window and
   * handed the title the same squeeze the phone measurement had just fixed. `@md`
   * measures the panel content box (`@container` in `WorkspaceScreen`) and, being
   * rem-based, doubles its threshold with the reader's text size.
   */
  it('gives the report title the whole row on a phone', async () => {
    serve([{ ...REPORT, title: 'Laporan Akhir Semester' }])
    renderPanel()

    const title = await screen.findByText('Laporan Akhir Semester')
    expect(title.className).toContain('w-full')
    expect(title.className).toContain('@md:flex-1')
  })
})

describe('creating a cheatsheet', () => {
  it('sends the scope, the depth and the sources, with no date when none was given', async () => {
    const served = serve()
    const user = userEvent.setup()
    renderPanel()

    await user.type(await screen.findByLabelText('Scope'), 'UTS Jaringan Komputer')
    await user.selectOptions(screen.getByLabelText('Depth'), 'thorough')
    const form = formWith('Scope')
    await user.click(within(form).getByRole('checkbox', { name: /Bab 3 Graf/ }))
    await user.click(within(form).getByRole('button', { name: 'Generate cheatsheet' }))

    // `exam_date` is null rather than an empty string: "no date" is a real state,
    // and an empty string is a date the server cannot parse.
    expect(served.bodies).toEqual([
      { scope: 'UTS Jaringan Komputer', exam_date: null, depth: 'thorough', source_ids: ['s1'] },
    ])
  })
})
