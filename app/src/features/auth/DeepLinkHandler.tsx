import { useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { useQueryClient } from '@tanstack/react-query'
import { Outlet } from 'react-router'

import { platform } from '@/platform'
import { GoogleCallbackScreen } from './GoogleCallbackScreen'
import type { DeepLinkIntent } from '@/platform'

/**
 * The one place a deep link enters the app.
 *
 * The shell classifies the incoming URL and hands over `{action, payload}`; the
 * SPA never parses a raw URL. Two actions are actionable here:
 *
 * - **`auth_ticket`** is the Google sign-in callback's single-use ticket, routed
 *   to the same screen the web build's `/auth/callback` route uses, so both entry
 *   points spend the ticket the same way.
 * - **`calendar`** is the Google *Calendar* consent return, and it carries a
 *   **status and a reason, never a token** (A234/A48): the refresh token was
 *   written server-side at the callback, and a deep link lands in browser history
 *   and the OS URL handler. So there is nothing here to store — only something to
 *   report, and something to re-read: `status=connected` invalidates the
 *   connections query so the panel shows the new connection without a reload,
 *   while `denied` and `error` carry the reason the student has to be told.
 *   Swallowing either would leave them pressing "Connect" again with no idea why
 *   it did nothing.
 *
 * Everything else is left for the screens that own it.
 *
 * `current()` is the cold-start pull (the shell was launched by the link, so no
 * event ever fires) and `onOpen` covers a link that arrives while the app is
 * already running. Both are needed; neither alone is enough.
 */
export function DeepLinkHandler() {
  const [ticket, setTicket] = useState<string | null>(null)
  const [calendarStatus, setCalendarStatus] = useState<CalendarReturn | null>(null)
  const queryClient = useQueryClient()

  useEffect(() => {
    let cancelled = false
    let unsubscribe: (() => void) | null = null

    function handle(intent: DeepLinkIntent) {
      if (intent.action === 'auth_ticket') {
        const value = intent.payload.ticket
        if (typeof value === 'string' && value.length > 0) setTicket(value)
        return
      }
      if (intent.action === 'calendar') {
        const status = intent.payload.params.status ?? ''
        // A connect that succeeded is not reported here: the panel itself shows
        // the connection, its address and its first sync, and a toast repeating
        // that would be a second, weaker answer to the same question. The read is
        // refreshed so the panel has the row the moment it renders.
        if (status === 'connected') {
          void queryClient.invalidateQueries({ queryKey: ['calendar'] })
          return
        }
        // A refusal is reported, because nothing else on the screen knows it
        // happened: the student was in a browser tab when it did.
        setCalendarStatus({
          status,
          reason: intent.payload.params.reason ?? '',
        })
      }
    }

    void platform.deepLinks
      .current()
      .then((intent) => {
        if (!cancelled && intent !== null) handle(intent)
      })
      .catch(() => {
        // A shell that cannot answer is the same as one with no launch link.
      })
    void platform.deepLinks
      .onOpen(handle)
      .then((off) => {
        // The subscription may resolve after the app has already torn the effect
        // down (a StrictMode remount does exactly this), so it is unsubscribed
        // here rather than left listening to a handler that no longer belongs to
        // the tree.
        if (cancelled) off()
        else unsubscribe = off
      })
      .catch(() => {
        // No deep-link channel: nothing to unsubscribe from.
      })

    return () => {
      cancelled = true
      unsubscribe?.()
    }
  }, [queryClient])

  if (ticket !== null) {
    return <GoogleCallbackScreen ticket={ticket} onLeave={() => setTicket(null)} />
  }
  return (
    <>
      {calendarStatus === null ? null : (
        <CalendarReturnNotice
          value={calendarStatus}
          onDismiss={() => setCalendarStatus(null)}
        />
      )}
      <Outlet />
    </>
  )
}

/** What the calendar consent return said. `status` is the server's own word. */
interface CalendarReturn {
  status: string
  reason: string
}

/**
 * The calendar consent return, reported over the app.
 *
 * A `role="status"` region with a dismiss control, not a toast that vanishes: the
 * student has just come back from a browser tab, and a reason that disappears
 * after three seconds is a reason they never read. The words are the honest ones —
 * "denied" says the access was refused rather than that something broke, because
 * they are different situations with different remedies.
 */
function CalendarReturnNotice({
  value,
  onDismiss,
}: {
  value: CalendarReturn
  onDismiss: () => void
}) {
  const { t } = useTranslation()
  const denied = value.status === 'denied'
  return (
    <div
      className={`note ${denied ? '' : 'note-danger'} mx-5 mt-4 flex items-start justify-between gap-3`}
      role="status"
    >
      <div className="flex min-w-0 flex-col gap-1">
        <p className="font-medium">
          {denied
            ? t('calendar.connections.google.denied')
            : t('calendar.connections.google.failed')}
        </p>
        {/* The server's own reason, in its own words. An empty one renders
            nothing rather than an empty line. */}
        {value.reason === '' ? null : (
          <p className="min-w-0 break-words text-small text-muted">{value.reason}</p>
        )}
      </div>
      <button type="button" className="btn btn-quiet shrink-0" onClick={onDismiss}>
        {t('actions.close')}
      </button>
    </div>
  )
}
