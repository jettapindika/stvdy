import { useEffect, useState } from 'react'
import { Outlet } from 'react-router'

import { platform } from '@/platform'
import { GoogleCallbackScreen } from './GoogleCallbackScreen'
import type { DeepLinkIntent } from '@/platform'

/**
 * The one place a deep link enters the app.
 *
 * The shell classifies the incoming URL and hands over `{action, payload}`; the
 * SPA never parses a raw URL. Only `auth_ticket` is actionable here: it is the
 * Google sign-in callback's single-use ticket, which is routed to the same
 * screen the web build's `/auth/callback` route uses, so both entry points spend
 * the ticket the same way. Everything else is left for the screens that own it.
 *
 * `current()` is the cold-start pull (the shell was launched by the link, so no
 * event ever fires) and `onOpen` covers a link that arrives while the app is
 * already running. Both are needed; neither alone is enough.
 */
export function DeepLinkHandler() {
  const [ticket, setTicket] = useState<string | null>(null)

  useEffect(() => {
    let cancelled = false
    let unsubscribe: (() => void) | null = null

    function handle(intent: DeepLinkIntent) {
      if (intent.action !== 'auth_ticket') return
      const value = intent.payload.ticket
      if (typeof value === 'string' && value.length > 0) setTicket(value)
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
  }, [])

  if (ticket !== null) {
    return <GoogleCallbackScreen ticket={ticket} onLeave={() => setTicket(null)} />
  }
  return <Outlet />
}
