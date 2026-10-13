import { useEffect, useRef } from 'react'
import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { Link, useSearchParams } from 'react-router'
import { useQueryClient } from '@tanstack/react-query'

import { asApiError } from '@/api/errors'
import { AUTH_ME_KEY, useResendVerification, useSignOut, useVerifyEmail } from '@/api/queries/auth'
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

export function VerifyEmailScreen() {
  const { t } = useTranslation()
  const [params] = useSearchParams()
  // An empty `?token=` is a malformed link, not a token to spend. Treating it as
  // one sends a request that can only fail, and shows the user "verifying" for
  // the round trip before telling them the link is dead.
  const rawToken = params.get('token')
  const token = rawToken !== null && rawToken.trim().length > 0 ? rawToken : null
  const session = useSession()
  const verify = useVerifyEmail()
  const resend = useResendVerification()
  const signOut = useSignOut()
  const queryClient = useQueryClient()
  const started = useRef(false)

  const signedIn = session.status === 'signed-in'

  // Resolved here rather than at module scope: copy needs a language, and there
  // is none until the component renders.
  const copy: Record<View, { title: string; subtitle: string }> = {
    'awaiting-link': {
      title: t('auth.verify.awaitingTitle'),
      subtitle: t('auth.verify.awaitingSubtitle'),
    },
    verifying: {
      title: t('auth.verify.verifyingTitle'),
      subtitle: t('auth.verify.verifyingSubtitle'),
    },
    verified: {
      title: t('auth.verify.verifiedTitle'),
      subtitle: t('auth.verify.verifiedSubtitle'),
    },
    failed: {
      title: t('auth.verify.failedTitle'),
      subtitle: t('auth.verify.failedSubtitle'),
    },
  }

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
          {t('auth.verify.resendSent')}
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
        {resend.isPending ? t('auth.verify.resendPending') : t('auth.verify.resend')}
      </button>
    </div>
  ) : null

  return (
    <AuthCard title={copy[view].title} subtitle={copy[view].subtitle}>
      {view === 'awaiting-link' && (
        <div className="flex flex-col gap-4">
          <p className="text-body text-muted">
            {signedIn
              ? t('auth.verify.sentTo', { email: session.user.email })
              : t('auth.verify.needLink')}
          </p>
          {signedIn ? (
            <>
              {/* The workspace is closed until the address is confirmed (A359), so there is
                  no link to it here. What the student needs instead is the two ways forward:
                  another link, or a different address — and the second one is why the
                  sign-out is on this screen at all. An unverified account that could not
                  sign out would be an account nobody can leave, and a mistyped address would
                  be permanent. */}
              <p className="text-label text-muted">{t('auth.verify.lockedUntilVerified')}</p>
              {resendBlock}
              <button
                type="button"
                className="btn btn-quiet w-full"
                disabled={signOut.isPending}
                onClick={() => {
                  signOut.mutate()
                }}
              >
                {t('auth.verify.useAnotherAddress')}
              </button>
            </>
          ) : (
            <Link to="/signin" className="btn btn-primary w-full">
              {t('auth.signIn.title')}
            </Link>
          )}
        </div>
      )}

      {view === 'verifying' && <LoadingState label={t('auth.verify.checking')} />}

      {view === 'verified' && (
        <div className="flex flex-col gap-4">
          <p className="note note-success" role="status">
            {signedIn ? t('auth.verify.verifiedSignedIn') : t('auth.verify.verifiedSignedOut')}
          </p>
          {/* The session's copy of the user is what the shell's gate reads (A359), and the
              token was spent a moment ago: until `/auth/me` answers, `email_verified` is still
              false and a link to the workspace would bounce straight back here. Waiting is the
              honest state — the verification has succeeded and the session has not caught up
              yet — and it also keeps the token from being spent twice, since a bounce would
              re-render this screen with `?token=` still in the URL. */}
          {signedIn && !session.user.email_verified ? (
            <LoadingState label={t('auth.verify.refreshingSession')} />
          ) : (
            <Link to={signedIn ? '/workspaces' : '/signin'} className="btn btn-primary w-full">
              {signedIn ? t('auth.verify.goToWorkspaces') : t('auth.signIn.title')}
            </Link>
          )}
        </div>
      )}

      {view === 'failed' && (
        <div className="flex flex-col gap-4">
          {linkRejected ? (
            <ErrorState
              title={t('auth.verify.linkGoneTitle')}
              reason={t('auth.verify.linkGoneReason')}
            />
          ) : (
            error !== null && <ApiErrorNotice error={error} />
          )}
          {signedIn ? (
            resendBlock
          ) : (
            <Link to="/signin" className="btn btn-primary w-full">
              {t('auth.verify.signInToRequest')}
            </Link>
          )}
        </div>
      )}
    </AuthCard>
  )
}
