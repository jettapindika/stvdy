import { useEffect, useRef } from 'react'
import { Link, useNavigate, useSearchParams } from 'react-router'

import { asApiError } from '@/api/errors'
import { useGoogleSignIn } from '@/api/queries/auth'
import { ApiErrorNotice, ErrorState, LoadingState } from '@/components/ui/States'
import { AuthCard } from './AuthLayout'

/**
 * The landing page for a completed Google sign-in.
 *
 * The server's OAuth callback redirects with a single-use `?ticket=...` and
 * never with tokens, because a deep link lands in browser history and the OS URL
 * handler and a credential placed there could not be revoked. This screen spends
 * that ticket exactly once and establishes the session.
 *
 * There are two ways in and they share this one screen: the deployed web build
 * returns to the `/auth/callback` route with the ticket in the query string, and
 * the native shell hands the SPA an `auth_ticket` intent which `DeepLinkHandler`
 * renders here. Doing the exchange in one place means the two entry points
 * cannot drift, and the "once" is guarded by a ref because the ticket is single
 * use: a re-render or a StrictMode double mount that spent it twice would leave
 * the user staring at a link that no longer works.
 */

export function GoogleCallbackRoute() {
  const [params] = useSearchParams()
  return <GoogleCallbackScreen ticket={params.get('ticket')} />
}

export function GoogleCallbackScreen({
  ticket,
  onLeave,
}: {
  ticket: string | null
  /**
   * Called when this screen is finished with: on a successful exchange (so a
   * host overlay can uncover the app it was covering) and when the user leaves a
   * failure state. The route does not need it, because navigating away unmounts
   * the screen; the native deep-link overlay does.
   */
  onLeave?: () => void
}) {
  const navigate = useNavigate()
  const google = useGoogleSignIn()
  const started = useRef(false)

  useEffect(() => {
    if (ticket === null || started.current) return
    started.current = true
    google.mutate(ticket, {
      onSuccess: () => {
        void navigate('/workspaces', { replace: true })
        onLeave?.()
      },
    })
    // Deliberately keyed on the ticket alone: it is spent once, and the mutation
    // object is new on every render.
  }, [ticket])

  const error = asApiError(google.error)

  if (ticket === null) {
    return (
      <AuthCard
        title="This sign-in link is incomplete"
        subtitle="Google sent you back without the one-time ticket that completes the sign-in."
      >
        <div className="flex flex-col gap-4">
          <ErrorState
            title="Nothing to complete"
            reason="Open the Google sign-in button again and finish it in one go. This page needs the ticket that Google returns with."
          />
          <Link to="/signin" className="btn btn-primary w-full" onClick={onLeave}>
            Back to sign in
          </Link>
        </div>
      </AuthCard>
    )
  }

  if (error !== null) {
    return (
      <AuthCard
        title="Google sign-in did not finish"
        subtitle="The sign-in could not be completed. Nothing was changed on your account."
      >
        <div className="flex flex-col gap-4">
          <ApiErrorNotice
            error={error}
            noun="signing in with Google"
            // An unknown, used, or expired ticket is the only 401 this route can
            // see, so it must not be described as an ended session.
            {...(error.code === 'unauthorized'
              ? {
                  title: 'That link no longer works',
                  reason:
                    'Sign-in tickets are single use and they expire after five minutes. Start the Google sign-in again.',
                }
              : {})}
          />
          <Link to="/signin" className="btn btn-primary w-full" onClick={onLeave}>
            Back to sign in
          </Link>
        </div>
      </AuthCard>
    )
  }

  return (
    <AuthCard
      title="Signing you in"
      subtitle="Finishing the Google sign-in you just started."
    >
      <LoadingState label="Completing sign-in" />
    </AuthCard>
  )
}
