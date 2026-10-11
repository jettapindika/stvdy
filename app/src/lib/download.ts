/**
 * Handing bytes to the platform as a file.
 *
 * The exports are fetched through the API layer rather than linked with an
 * `<a href>`, because the export routes need the bearer token and a navigation
 * cannot carry a header. That leaves the client holding the document, and this is
 * the one place that turns it back into something the user can keep.
 *
 * The anchor is appended to the document before the click and removed after it:
 * Firefox ignores a click on an anchor that is not in the document, which would
 * look like a download button that silently does nothing.
 *
 * The object URL is revoked on the next task rather than immediately. Revoking it
 * in the same tick can cancel a download that has not started reading yet — the
 * browser resolves the URL asynchronously — and the failure is a truncated or
 * missing file, which is exactly the sort of defect that never reproduces.
 */
export function saveBlob(blob: Blob, filename: string): void {
  const url = URL.createObjectURL(blob)
  const anchor = document.createElement('a')
  anchor.href = url
  anchor.download = filename
  anchor.rel = 'noopener'
  document.body.append(anchor)
  anchor.click()
  anchor.remove()
  setTimeout(() => {
    URL.revokeObjectURL(url)
  }, 0)
}
