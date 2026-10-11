import { useEffect, useRef } from 'react'
import type { ReactNode } from 'react'
import { Link, useSearchParams } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'

import { asApiError } from '@/api/errors'
import { AUTH_ME_KEY, useResendVerification, useVerifyEmail } from '@/api/queries/auth'
import { ApiErrorNotice, ErrorState, LoadingState } from '@/components/ui/States'
import { AuthCard } from './AuthLayout'
import { useSession } from './useSession'

/**
 * Email verification, both halves of it.
 *
 * With `?token=` this is the page the emailed link opens, and it spends the token
 * exactly once. That "once" is guarded by a ref rather than by the render, because
 * the token is single use: a second POST from a re-render or a StrictMode double
 * mount would consume it and leave the user staring at a link that no longer
 * works. (The endpoint is idempotent server side, but relying on that to paper
 * over a client bug is the wrong order.)
 *
 * Without a token it is where signup lands, and it says the one thing that user
 * needs: check your inbox, and here is how to get another link if it does not
 * arrive. Verification links expire, and a dead link with no way forward is the
 * dead end R-24 is about.
 */

type View = 'awaiting-link' | 'verifying' | 'verified' | 'failed'

const VIEW_COPY: Record<View, { title: string; subtitle: string }> = {
  'awaiting-link': {
    title: 'Check your email',
    subtitle: 'One more step before your account is fully set up.',
  },
  verifying: {
    title: 'Verifying your email',
    subtitle: 'Checking this link with the server.',
  },
  verified: {
    title: 'Email verified',
    subtitle: 'Your address is confirmed. Nothing else is needed from you.',
  },
  failed: {
    title: 'Verification failed',
    subtitle: 'This link could not be used.',
  },
}

export function VerifyEmailScreen() {
  const [params] = useSearchParams()
  // An empty `?token=` is a malformed link, not a token to spend. Treating it as
  // one sends a request that can only fail, and shows the user "verifying" for
  // the round trip before telling them the link is dead.
  const rawToken = params.get('token')
  const token = rawToken !== null && rawToken.trim().length > 0 ? rawToken : null
  const session = useSession()
  const verify = useVerifyEmail()
  const resend = useResendVerification()
  const queryClient = useQueryClient()
  const started = useRef(false)

  const signedIn = session.status === 'signed-in'

  useEffect(() => {
    if (token === null || started.current) return
    started.current = true
    verify.mutate(token, {
      onSuccess: () => {
        // The cached user still says `email_verified: false`, and every screen
        // that shows the banner reads that field.
        if (signedIn) void queryClient.invalidateQueries({ queryKey: AUTH_ME_KEY })
      },
    })
    // Deliberately keyed on the token alone: it is spent once, and the mutation
    // object is new on every render.
  }, [token])

  const error = asApiError(verify.error)
  const resendError = asApiError(resend.error)
  // The link itself was refused: expired, already used, or never issued. Those
  // are the only reasons a verification token fails, so they get one message
  // rather than a distinction the server does not make.
  const linkRejected =
    error !== null && (error.code === 'validation' || error.code === 'not_found')

  const view: View =
    token === null ? 'awaiting-link' : error !== null ? 'failed' : verify.isSuccess ? 'verified' : 'verifying'

  const resendBlock: ReactNode = signedIn ? (
    <div className="flex flex-col gap-2">
      {resend.isSuccess && (
        <p className="note note-success" role="status">
          A new link is on its way. It can take a minute to arrive.
        </p>
      )}
      {resendError !== null && <ApiErrorNotice error={resendError} />}
      <button
        type="button"
        className="btn btn-secondary w-full"
        disabled={resend.isPending}
        onClick={() => {
          resend.mutate()
        }}
      >
        {resend.isPending ? 'Sending a new link' : 'Send a new link'}
      </button>
    </div>
  ) : null

  return (
    <AuthCard title={VIEW_COPY[view].title} subtitle={VIEW_COPY[view].subtitle}>
      {view === 'awaiting-link' && (
        <div className="flex flex-col gap-4">
          <p className="text-sm text-ink-muted">
            {signedIn
              ? `We sent a verification link to ${session.user.email}. Open it to confirm that address.`
              : 'This page needs the link from your verification email. Open the newest one, or sign in to request another.'}
          </p>
          {signedIn ? (
            <>
              {resendBlock}
              <Link to="/workspaces" className="btn btn-primary w-full">
                Go to your workspaces
              </Link>
            </>
          ) : (
            <Link to="/signin" className="btn btn-primary w-full">
              Sign in
            </Link>
          )}
        </div>
      )}

      {view === 'verifying' && <LoadingState label="Checking this verification link" />}

      {view === 'verified' && (
        <div className="flex flex-col gap-4">
          <p className="note note-success" role="status">
            {signedIn
              ? 'Your email address is verified.'
              : 'Your email address is verified. Sign in to continue.'}
          </p>
          <Link to={signedIn ? '/workspaces' : '/signin'} className="btn btn-primary w-full">
            {signedIn ? 'Go to your workspaces' : 'Sign in'}
          </Link>
        </div>
      )}

      {view === 'failed' && (
        <div className="flex flex-col gap-4">
          {linkRejected ? (
            <ErrorState
              title="This link no longer works"
              reason="Verification links are single use and they expire. Request a new one and open the newest email, because older links stop working as soon as a newer one is issued."
            />
          ) : (
            error !== null && <ApiErrorNotice error={error} />
          )}
          {signedIn ? (
            resendBlock
          ) : (
            <Link to="/signin" className="btn btn-primary w-full">
              Sign in to request a new link
            </Link>
          )}
        </div>
      )}
    </AuthCard>
  )
}
