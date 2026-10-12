import { useEffect, useState } from 'react'

import { platform, type SyncReport } from '@/platform'

/**
 * The sync status, as the shell reports it (A187).
 *
 * Two reads, because the status is both a fact and an event. `status()` is the
 * fact at mount — a control that waited for the next pass would show nothing on a
 * quiet device — and `onStatus` is every change after it, which is what makes the
 * control live without polling. The shell derives the report from stored facts
 * (the outbox, the last pass's outcome, the stored `last_sync_at`), so nothing
 * here reads a clock or invents a value: this hook only carries what it was told.
 *
 * A failed read is reported as `failed` rather than as a default report. The one
 * thing this hook must never do is answer `synced` because it could not find out:
 * that is a claim about work the app has not verified, and it is exactly the lie
 * the status control exists to avoid.
 */
export function useSyncStatus(): { report: SyncReport | null; failed: boolean } {
  const [report, setReport] = useState<SyncReport | null>(null)
  const [failed, setFailed] = useState(false)

  useEffect(() => {
    let cancelled = false
    let unsubscribe: (() => void) | null = null

    void platform.sync
      .status()
      .then((next) => {
        if (!cancelled) setReport(next)
      })
      .catch(() => {
        if (!cancelled) setFailed(true)
      })

    void platform.sync
      .onStatus((next) => {
        // A status that arrived is proof the channel works, so a failure before
        // it is cleared rather than left over the fresh report.
        setReport(next)
        setFailed(false)
      })
      .then((off) => {
        // The subscription may resolve after the control unmounted (a StrictMode
        // remount does exactly this), so it is dropped here rather than left
        // calling a handler that no longer belongs to the tree.
        if (cancelled) off()
        else unsubscribe = off
      })
      .catch(() => {
        if (!cancelled) setFailed(true)
      })

    return () => {
      cancelled = true
      unsubscribe?.()
    }
  }, [])

  return { report, failed }
}
