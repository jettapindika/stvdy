import { useEffect, useRef } from 'react'
import { useTranslation } from 'react-i18next'
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
  const { t } = useTranslation()
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
        title={t('auth.googleCallback.incompleteTitle')}
        subtitle={t('auth.googleCallback.incompleteSubtitle')}
      >
        <div className="flex flex-col gap-4">
          <ErrorState
            title={t('auth.googleCallback.nothingToComplete')}
            reason={t('auth.googleCallback.nothingToCompleteReason')}
          />
          <Link to="/signin" className="btn btn-primary w-full" onClick={onLeave}>
            {t('auth.googleCallback.backToSignIn')}
          </Link>
        </div>
      </AuthCard>
    )
  }

  if (error !== null) {
    return (
      <AuthCard
        title={t('auth.googleCallback.failedTitle')}
        subtitle={t('auth.googleCallback.failedSubtitle')}
      >
        <div className="flex flex-col gap-4">
          <ApiErrorNotice
            error={error}
            noun={t('auth.googleCallback.noun')}
            // An unknown, used, or expired ticket is the only 401 this route can
            // see, so it must not be described as an ended session.
            {...(error.code === 'unauthorized'
              ? {
                  title: t('auth.googleCallback.linkGoneTitle'),
                  reason: t('auth.googleCallback.linkGoneReason'),
                }
              : {})}
          />
          <Link to="/signin" className="btn btn-primary w-full" onClick={onLeave}>
            {t('auth.googleCallback.backToSignIn')}
          </Link>
        </div>
      </AuthCard>
    )
  }

  return (
    <AuthCard
      title={t('auth.googleCallback.pendingTitle')}
      subtitle={t('auth.googleCallback.pendingSubtitle')}
    >
      <LoadingState label={t('auth.googleCallback.loading')} />
    </AuthCard>
  )
}
