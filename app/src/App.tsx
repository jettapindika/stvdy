import { useCallback, useEffect, useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Navigate, Outlet, useLocation } from 'react-router'

import { CommandPalette } from '@/features/palette/CommandPalette'
import { Shell } from './components/layout/Shell'
import { LoadingState } from './components/ui/States'
import { MotionSurface } from './components/ui/MotionSurface'
import { useSession } from './features/auth/useSession'
import { usePushRegistration } from './features/notifications/registerPush'
import { useLocalNotifications } from './features/reminders/useLocalNotifications'
import { platform } from './platform'

/**
 * The prefixes an unverified session may still reach (A359).
 *
 * Settings, because it holds the account's own surface: the device list (revoke the machine
 * you are on), the data export, and the deletion. A student who typed an address they cannot
 * receive mail at has to be able to revoke a device, take their data out and leave — that is
 * a promise in `docs/EULA.md` § 9 and a store rule, and a gate that closed it would be
 * trapping them rather than gating them. Every other route refuses them server-side, so
 * reaching one only shows the refusal it would have shown anyway.
 */
const ACCOUNT_ROUTES = ['/settings']

/**
 * The signed-in shell, and the two gates in front of it.
 *
 * The gates are here rather than on each screen because every route below needs the same
 * thing, and a screen that forgets to check is a screen that fires requests with no token and
 * renders their 401s as if they were failures of the workspace. One gate cannot be forgotten.
 *
 * `loading` is its own state, not a synonym for signed out. On a cold start the
 * stored refresh token is still being exchanged, and treating that moment as
 * "signed out" would flash the sign-in screen at every launch, which reads as
 * having been logged out.
 *
 * **The second gate is verification** (A359). An unverified session holds a valid token — the
 * account exists and `GET /auth/me` answers — but the workspace is closed to it, so rendering
 * the shell would be twenty screens of `403 email_not_verified`. It is sent to `/verify`, which
 * is where the emailed link lands and the only screen with the remedy on it: resend the link,
 * or sign out and use a different address. The server refuses the same routes regardless; this
 * gate is what keeps the refusal from being the whole interface.
 *
 * **The command palette is mounted here, not in the top bar.** Its shortcut is a
 * window-level binding (`⌘K` / `Ctrl K`), and a binding that only worked while the
 * top bar had focus would not be a shortcut. Mounting it in the gate also means
 * the palette exists on a phone, where the top bar does not — the phone header's
 * search action opens the same dialog.
 */
export function App() {
  const { t } = useTranslation()
  const session = useSession()
  const signedIn = session.status === 'signed-in'
  const location = useLocation()
  const [paletteOpen, setPaletteOpen] = useState(false)
  const previousPath = useRef(location.pathname)
  // The local schedule is mounted here rather than on a screen, because it is a
  // property of *this device* and not of any one route: a reminder has to be
  // scheduled whether or not the student opens Today. The hook gates itself to
  // ios/android (see its own doc comment — on web and desktop `schedule` delivers
  // immediately, so an ungated mount notifies on every page load), and `signedIn`
  // is the caller's half of the gate: with no session there is no account timezone
  // and no rows, and firing the reads anyway would be four 401s on the sign-in
  // screen.
  useLocalNotifications(signedIn)
  // The push token, registered against this device's row once there is a session
  // (A329). The hook is a no-op where the platform has no push capability — the web
  // build and desktop — and it re-registers on every token the OS re-issues.
  usePushRegistration(signedIn)

  const openPalette = useCallback(() => setPaletteOpen(true), [])
  const closePalette = useCallback(() => setPaletteOpen(false), [])

  useEffect(() => {
    // The shortcut is read from the platform adapter so the same binding is
    // described and handled from one place: `shortcutLabel('K')` renders `⌘K` on
    // macOS and `Ctrl K` elsewhere, and `matchesShortcut` is the corresponding
    // test. A screen that hardcoded `metaKey` would be wrong on Windows, where the
    // modifier is Control.
    const onKeyDown = (event: KeyboardEvent) => {
      if (platform.matchesShortcut(event, 'K')) {
        event.preventDefault()
        setPaletteOpen((open) => !open)
      }
    }
    window.addEventListener('keydown', onKeyDown)
    return () => window.removeEventListener('keydown', onKeyDown)
  }, [])

  useEffect(() => {
    if (previousPath.current === location.pathname) return
    previousPath.current = location.pathname
    if (!signedIn) return
    document.getElementById('main')?.focus({ preventScroll: true })
  }, [location.pathname, signedIn])

  if (session.status === 'loading') {
    return (
      <div className="flex min-h-[100dvh] items-start justify-center bg-bg pt-safe-top">
        <LoadingState label={t('app.session.restoring')} />
      </div>
    )
  }

  if (session.status === 'signed-out') return <Navigate to="/signin" replace />

  // The verification gate (A359). `replace`, not a push: the route the student was trying to
  // reach is not one they can come back to until the address is confirmed, and leaving it in
  // history would make Back bounce off this gate forever.
  const onAccountRoute = ACCOUNT_ROUTES.some((prefix) => location.pathname.startsWith(prefix))
  if (!session.user.email_verified && !onAccountRoute) {
    return <Navigate to="/verify" replace />
  }

  return (
    <Shell onOpenPalette={openPalette}>
      <MotionSurface key={location.pathname} className="flex min-h-0 min-w-0 flex-1 flex-col">
        <Outlet />
      </MotionSurface>
      <CommandPalette open={paletteOpen} onClose={closePalette} />
    </Shell>
  )
}
