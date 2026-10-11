import { Navigate, Outlet } from 'react-router'

import { Shell } from './components/layout/Shell'
import { LoadingState } from './components/ui/States'
import { useSession } from './features/auth/useSession'

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
 */
export function App() {
  const session = useSession()

  if (session.status === 'loading') {
    return (
      <div className="flex min-h-[100dvh] items-start justify-center bg-paper pt-safe-top">
        <LoadingState label="Restoring your session" />
      </div>
    )
  }

  if (session.status === 'signed-out') return <Navigate to="/signin" replace />

  return (
    <Shell>
      <Outlet />
    </Shell>
  )
}
