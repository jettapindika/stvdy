import { useCallback, useEffect, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Navigate, Outlet } from 'react-router'

import { CommandPalette } from '@/features/palette/CommandPalette'
import { Shell } from './components/layout/Shell'
import { LoadingState } from './components/ui/States'
import { useSession } from './features/auth/useSession'
import { usePushRegistration } from './features/notifications/registerPush'
import { useLocalNotifications } from './features/reminders/useLocalNotifications'
import { platform } from './platform'

/**
 * The signed-in shell, and the gate in front of it.
 *
 * The gate is here rather than on each screen because every route below it needs
 * the same thing, and a screen that forgets to check is a screen that fires
 * requests with no token and renders their 401s as if they were failures of the
 * workspace. One gate cannot be forgotten.
 *
 * `loading` is its own state, not a synonym for signed out. On a cold start the
 * stored refresh token is still being exchanged, and treating that moment as
 * "signed out" would flash the sign-in screen at every launch, which reads as
 * having been logged out.
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
  const [paletteOpen, setPaletteOpen] = useState(false)
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

  if (session.status === 'loading') {
    return (
      <div className="flex min-h-[100dvh] items-start justify-center bg-bg pt-safe-top">
        <LoadingState label={t('app.session.restoring')} />
      </div>
    )
  }

  if (session.status === 'signed-out') return <Navigate to="/signin" replace />

  return (
    <Shell onOpenPalette={openPalette}>
      <Outlet />
      <CommandPalette open={paletteOpen} onClose={closePalette} />
    </Shell>
  )
}
