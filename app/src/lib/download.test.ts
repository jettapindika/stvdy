import { afterEach, describe, expect, it, vi } from 'vitest'

import { saveBlob } from './download'

/**
 * Handing a fetched document to the platform as a file.
 *
 * The exports arrive as bytes — the export routes need the bearer token, so they
 * cannot be linked with an `<a href>` — and this is the one place that turns them
 * back into something the student can keep. Two details of it are behavioural
 * rather than stylistic, and both are the kind that only shows up on one browser
 * or under load:
 *
 * - the anchor has to be **in the document** when it is clicked, or Firefox
 *   ignores the click and the button looks like it did nothing;
 * - the object URL has to be revoked, or every download leaks the document for
 *   the life of the tab.
 */
afterEach(() => {
  vi.unstubAllGlobals()
})

describe('saving a fetched document', () => {
  it('clicks an anchor in the document, with the filename and the object URL', () => {
    const blob = new Blob(['# Jaringan'], { type: 'text/markdown' })
    const created: Blob[] = []
    vi.stubGlobal(
      'URL',
      Object.assign(URL, {
        createObjectURL: (value: Blob) => {
          created.push(value)
          return 'blob:report'
        },
        revokeObjectURL: vi.fn(),
      }),
    )

    const clicked: HTMLAnchorElement[] = []
    const click = vi
      .spyOn(HTMLAnchorElement.prototype, 'click')
      .mockImplementation(function (this: HTMLAnchorElement) {
        // Read at click time, which is also when the browser would: the anchor is
        // asserted while it is still attached, and it carries the download name.
        clicked.push(this)
        expect(document.body.contains(this)).toBe(true)
      })

    saveBlob(blob, 'report.md')

    expect(clicked).toHaveLength(1)
    expect(clicked[0]?.download).toBe('report.md')
    expect(clicked[0]?.getAttribute('href')).toBe('blob:report')
    expect(created).toEqual([blob])
    // And the anchor is gone afterwards: a leftover one is a second clickable
    // control in the document that nobody can see.
    expect(document.body.contains(clicked[0] as HTMLAnchorElement)).toBe(false)

    click.mockRestore()
  })

  it('revokes the object URL, but not before the browser has read it', async () => {
    vi.stubGlobal(
      'URL',
      Object.assign(URL, {
        createObjectURL: () => 'blob:report',
        revokeObjectURL: vi.fn(),
      }),
    )
    vi.spyOn(HTMLAnchorElement.prototype, 'click').mockImplementation(() => {})

    saveBlob(new Blob(['%PDF-1.4']), 'report.pdf')

    // Not synchronously: the browser resolves the URL asynchronously, and revoking
    // it in the same tick can cancel a download that has not started reading,
    // which produces a truncated file rather than an error.
    expect(URL.revokeObjectURL).not.toHaveBeenCalled()
    await vi.waitFor(() => {
      expect(URL.revokeObjectURL).toHaveBeenCalledWith('blob:report')
    })
  })
})
