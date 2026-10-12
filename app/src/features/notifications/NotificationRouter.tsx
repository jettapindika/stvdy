import { useEffect, useState } from 'react'
import { useNavigate } from 'react-router'

import { useSession } from '@/features/auth/useSession'
import { platform } from '@/platform'
import type { NotificationTap } from '@/platform'

/**
 * Where a tapped notification lands (A320, A316).
 *
 * The shell classifies a *link* and hands the SPA `{action, payload}`
 * (`platform.deepLinks`); a *notification tap* arrives as the notification plugin's
 * own click event, which is a different channel with a different payload. This module
 * is that channel's one handler.
 *
 * **A tap is the only surface a push is guaranteed to produce** (A316). A silent push
 * may be throttled, dropped, or never delivered while the app is force-quit; a tap is
 * a launch or a foreground transition, which the app always sees. So this is not an
 * optimisation over a push handler — it is the only part of the push path that can be
 * relied on, which is why the route mapping is a pure function with its own tests.
 */

/**
 * What a tap is about.
 *
 * The payload is the server's `data` map — for a reminder, `reminder_id` and `kind`
 * (`crates/server/src/jobs/handlers/reminders.rs`) — so the route is derived from the
 * *same* keys the server sent rather than from a second vocabulary invented here.
 */
export type NotificationTarget =
  | { kind: 'workspace'; id: string }
  | { kind: 'task' }
  /** A reminder, whose subject decides the screen: an event or a task. */
  | { kind: 'reminder'; subject: 'task' | 'event' | 'review' }

/**
 * The target a notification's payload names, or null when it names none.
 *
 * The order is most-specific-first and that is the contract: a payload carrying both
 * a `workspace_id` and a `task_id` is about the workspace (the screen that contains
 * the task), and a payload carrying nothing this app understands is not routed at
 * all — a tap that guessed a destination would be worse than one that stayed put,
 * because the student would be moved away from where they were for no reason.
 *
 * A **local** notification has no `data` at all (it is one of
 * `NotificationsApi.schedule`'s, shown by the OS from a plan the device computed), so
 * `null` is a case that really occurs rather than a defensive branch.
 */
export function notificationTarget(
  data: Record<string, string> | null | undefined,
): NotificationTarget | null {
  if (data == null) return null

  const workspaceId = nonBlank(data.workspace_id)
  if (workspaceId !== null) return { kind: 'workspace', id: workspaceId }

  if (nonBlank(data.task_id) !== null) return { kind: 'task' }

  if (nonBlank(data.reminder_id) !== null) {
    // The server's three kinds (`crates/core/src/reminders.rs`). `cards_due` is
    // day-granular and derived from the deck, so there is no subject id to open —
    // the review queue on Today is where it belongs.
    if (data.kind === 'event_soon') return { kind: 'reminder', subject: 'event' }
    if (data.kind === 'task_due') return { kind: 'reminder', subject: 'task' }
    return { kind: 'reminder', subject: 'review' }
  }

  return null
}

/** The path a target resolves to. Every one of these is a route `router.tsx` registers. */
export function notificationPath(target: NotificationTarget): string {
  switch (target.kind) {
    case 'workspace':
      return `/workspaces/${target.id}`
    case 'task':
      return '/tasks'
    case 'reminder':
      return target.subject === 'event'
        ? '/calendar'
        : target.subject === 'task'
          ? '/tasks'
          : '/today'
  }
}

/** The path a tap resolves to, or null when the tap names no destination. */
export function notificationRouteFor(tap: NotificationTap): string | null {
  const target = notificationTarget(tap.data)
  return target === null ? null : notificationPath(target)
}

/** A payload value that is present and not blank, or null. */
function nonBlank(value: string | undefined): string | null {
  if (value === undefined) return null
  const trimmed = value.trim()
  return trimmed === '' ? null : trimmed
}

/**
 * The one place a notification tap enters the app.
 *
 * **The tap is buffered, and the buffer is the point.** Two windows have to be
 * covered, and they are different mechanisms:
 *
 * 1. **Before a listener exists.** A tap that *launched* the app arrives while the
 *    webview is still booting, so the plugin holds it and replays it when a listener
 *    registers (`set_click_listener_active` — see the mobile adapter). That is the
 *    notification half of the shell's `deep_link_current`, and it is the plugin's.
 * 2. **Before there is a session.** A cold start replays that tap before
 *    `useSession` has exchanged the stored refresh token, so navigating on arrival
 *    would land on `/signin` and the tap would be spent on a screen that redirects
 *    away. The tap is therefore held in state and spent when the session arrives —
 *    which is this component's half, and the reason it is not a bare `navigate` call
 *    in the subscription.
 *
 * A tap that arrives while the app is running is spent immediately: the session is
 * already there.
 *
 * Where the capability is absent — every browser and every laptop — there is no
 * subscription to make and this component renders nothing at all.
 *
 * It renders **nothing**, not an outlet: it is mounted beside `DeepLinkHandler` at the
 * root route, and the outlet that shows the screens is that handler's. A second outlet
 * here would be a second place the route tree could be rendered from.
 */
export function NotificationRouter() {
  const navigate = useNavigate()
  const session = useSession()
  const signedIn = session.status === 'signed-in'
  const [pending, setPending] = useState<string | null>(null)

  useEffect(() => {
    const push = platform.push
    if (push === undefined) return

    let cancelled = false
    let off: (() => void) | null = null

    void push
      .onNotification((tap) => {
        const path = notificationRouteFor(tap)
        // A tap with no destination this app understands leaves the student where
        // they are. Moving them somewhere arbitrary would be worse than the tap
        // doing nothing, and this is a real case rather than a defensive one: the
        // plugin emits the same event for a local notification, which carries no
        // payload at all.
        if (path !== null) setPending(path)
      })
      .then((stop) => {
        // The subscription may resolve after the tree has already torn this effect
        // down (a StrictMode remount does exactly this), so it is unsubscribed here
        // rather than left listening to a handler that no longer belongs to it.
        if (cancelled) stop()
        else off = stop
      })
      .catch(() => {
        // No notification channel on this build: nothing to unsubscribe from.
      })

    return () => {
      cancelled = true
      off?.()
    }
  }, [])

  useEffect(() => {
    if (pending === null || !signedIn) return
    setPending(null)
    navigate(pending)
  }, [navigate, pending, signedIn])

  return null
}
