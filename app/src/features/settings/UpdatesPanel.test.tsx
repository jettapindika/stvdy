import { screen } from '@testing-library/react'
import userEvent from '@testing-library/user-event'
import { afterEach, describe, expect, it, vi } from 'vitest'

import { installFakePlatform } from '@/test/harness'
import { renderWithProviders } from '@/test/render'
import type { PlatformKind, UpdateProgress, UpdateStatus } from '@/platform/types'
import { UpdatesPanel } from './UpdatesPanel'

/**
 * The Updates panel: three states, and the honesty constraint that makes it safe.
 *
 * The panel is the one screen in this phase, and the risk it carries is not a
 * crash. It is that the app says something about a release it was not told — a
 * "what's new" line composed from nothing, a download percentage computed from a
 * byte count that may not exist, or a "security update" label no manifest carried.
 * Roadmap gate 7(a) is the rule; these tests are how it is enforced.
 *
 * The other half is the failure that reads as success. A check that did not finish
 * must never render "You are up to date": a student who cannot detect that is a
 * student who believes they are patched when they are not.
 */

const AVAILABLE: UpdateStatus = {
  kind: 'available',
  current: '0.1.0',
  version: '0.2.0',
  notes: 'Fixes the PDF export.\nAdds Bahasa Indonesia to the planner.',
}

function setup(kind: PlatformKind, status: UpdateStatus) {
  const checkForUpdate = vi.fn(async () => status)
  const installUpdate = vi.fn(async (onProgress: (progress: UpdateProgress) => void) => {
    onProgress({ phase: 'downloading' })
    onProgress({ phase: 'installing' })
    onProgress({ phase: 'restart-required' })
  })
  installFakePlatform(kind, { update: { checkForUpdate, installUpdate } })
  return { checkForUpdate, installUpdate }
}

afterEach(() => {
  vi.unstubAllGlobals()
})

describe('the updates panel', () => {
  it('renders nothing on the web, where there is no installer', () => {
    setup('web', { kind: 'unsupported' })
    const { container } = renderWithProviders(<UpdatesPanel />)

    // Absent, not disabled. A greyed-out "Check for updates" in a browser tab is a
    // promise the platform cannot keep, and a student who sees it looks for the
    // reason it is off.
    expect(container).toBeEmptyDOMElement()
    expect(screen.queryByRole('button')).toBeNull()
  })

  it('renders nothing on a phone, where updates ship through the stores', () => {
    setup('ios', { kind: 'unsupported' })
    const { container } = renderWithProviders(<UpdatesPanel />)
    expect(container).toBeEmptyDOMElement()

    setup('android', { kind: 'unsupported' })
    const android = renderWithProviders(<UpdatesPanel />)
    expect(android.container).toBeEmptyDOMElement()
  })

  it('shows the version and the notes exactly as the manifest carried them', async () => {
    const user = userEvent.setup()
    const { checkForUpdate } = setup('macos', AVAILABLE)
    renderWithProviders(<UpdatesPanel />)

    await user.click(screen.getByRole('button', { name: 'Check for updates' }))
    expect(checkForUpdate).toHaveBeenCalledTimes(1)

    // The two facts the manifest carried, and the version being replaced. Both
    // versions are real: `current` is what the app is running, `version` is what
    // the endpoint announced.
    expect(
      await screen.findByText('Version 0.2.0 is available. You are running 0.1.0.'),
    ).toBeInTheDocument()

    // Verbatim, including the line break the release author wrote.
    const notes = screen.getByText(/Fixes the PDF export\./)
    expect(notes.textContent).toBe(AVAILABLE.kind === 'available' ? AVAILABLE.notes : '')
  })

  it('never invents a percentage, a size or a security label', async () => {
    const user = userEvent.setup()
    setup('macos', AVAILABLE)
    renderWithProviders(<UpdatesPanel />)

    await user.click(screen.getByRole('button', { name: 'Check for updates' }))
    await screen.findByText(/Version 0.2.0 is available/)

    // Gate 7(a). The plugin's download events make `contentLength` optional, so a
    // percentage is real on some downloads and invented on others — and a
    // percentage, a megabyte figure and "security update" are all claims about
    // data this app was never given.
    const panel = document.body.textContent ?? ''
    expect(panel).not.toMatch(/%/)
    expect(panel).not.toMatch(/\d+\s?(MB|KB|GB|kB)/i)
    expect(panel).not.toMatch(/security (update|patch|fix)/i)
    expect(panel).not.toMatch(/what's new/i)
  })

  it('offers the update and never applies one on its own', async () => {
    const user = userEvent.setup()
    const { installUpdate } = setup('macos', AVAILABLE)
    renderWithProviders(<UpdatesPanel />)

    // A check is a click, not a mount effect: an app that downloads on startup is
    // an app that reaches the network before the student asked it to.
    expect(screen.queryByRole('button', { name: 'Download and install' })).toBeNull()

    await user.click(screen.getByRole('button', { name: 'Check for updates' }))
    const install = await screen.findByRole('button', { name: 'Download and install' })
    expect(installUpdate).not.toHaveBeenCalled()

    await user.click(install)
    expect(installUpdate).toHaveBeenCalledTimes(1)
    // Labelled as the update it is: never a "fix" and never a "security patch",
    // because the manifest did not say either (gate 7e).
    expect(await screen.findByText('Installed. Reopen Stvdy to use the new version.')).toBeInTheDocument()
  })

  it('reports a failed check as a failure, never as "up to date"', async () => {
    const user = userEvent.setup()
    setup('macos', { kind: 'error', message: 'the update endpoint is unreachable' })
    renderWithProviders(<UpdatesPanel />)

    await user.click(screen.getByRole('button', { name: 'Check for updates' }))

    // The one bug this panel can have that a student cannot detect: a check that
    // did not run while the screen reads "You are up to date". The failure names
    // the reason and the check stays available, so the remedy is the same button.
    expect(
      await screen.findByText('The check did not finish: the update endpoint is unreachable'),
    ).toBeInTheDocument()
    expect(screen.queryByText(/up to date/i)).toBeNull()
    expect(screen.getByRole('button', { name: 'Check for updates' })).toBeEnabled()
  })

  it('says which version is current when there is nothing to install', async () => {
    const user = userEvent.setup()
    setup('macos', { kind: 'up-to-date', current: '0.1.0' })
    renderWithProviders(<UpdatesPanel />)

    await user.click(screen.getByRole('button', { name: 'Check for updates' }))
    // The version comes from the check, not from a constant baked into the screen:
    // a hardcoded string would be right until the first release and wrong after.
    expect(
      await screen.findByText('You are up to date. This is version 0.1.0.'),
    ).toBeInTheDocument()
  })
})
