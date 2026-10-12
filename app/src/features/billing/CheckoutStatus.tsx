import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { Icon, type IconName } from '@/components/ui/Icon'
import type { CheckoutState } from '@/features/billing/checkout'

/**
 * The checkout screen's state, in words.
 *
 * Six states and every one of them is **spoken**: a heading, a sentence saying
 * what it means for the money, and the one action that state offers. The chip's
 * tone repeats what the words already say, which is the accessibility gate's rule
 * that no state is carried by colour alone (5d) — and it matters most on exactly
 * this screen, because a QR code screen is the natural place for a status that is
 * only a colour. A student who cannot tell the indigo "waiting" pill from the green
 * "paid" pill must still be able to read which one they are in.
 *
 * The five spec states are here, plus `offline`, and the distinctions between them
 * are the point:
 *
 * - **`waiting`** is the one that carries the weight. It is what a student sees
 *   when the webhook was lost, when the gateway is slow, and when the money has
 *   genuinely not moved — and it must not read as either of the other two. The
 *   copy says the fact ("we have not heard from the bank yet"), that the page
 *   updates itself, and nothing about failure or success.
 * - **`paid`** never renders a QR again (asserted in `checkout.test.ts`), because
 *   the code is dead the moment it is paid and a second scan is how a student pays
 *   twice.
 * - **`expired`** offers a new code; **`review`** deliberately does not, because a
 *   mismatch is waiting on a human (A294) and a retry there would be a second
 *   charge for a payment already being looked at.
 * - **`offline`** is this client's own state, and it says what is true: the code
 *   may still be valid, so nothing was lost.
 *
 * The region is `role="status"` with `aria-live="polite"`, so a student using a
 * screen reader hears the change from waiting to paid without hunting for it —
 * which is the whole reason the poll exists.
 */
export function CheckoutStatus({
  state,
  countdownLabel,
  onNewCode,
  onRetry,
}: {
  state: CheckoutState
  /** The rendered `mm:ss`, or null when the state has no countdown to show. */
  countdownLabel: string | null
  /** Offered on `expired` only. */
  onNewCode?: () => void
  /** Offered on `offline` only: the same request, again. */
  onRetry?: () => void
}) {
  const { t } = useTranslation()

  // The words, the tone and the mark for each state, in one table so a state
  // cannot be added to `CheckoutState` without the screen deciding what it says.
  const shown: Record<CheckoutState, { icon: IconName; chip: string; title: string; detail: string }> = {
    waiting: {
      icon: 'clock',
      chip: 'chip chip-working',
      title: t('checkout.state.waiting.title'),
      detail: t('checkout.state.waiting.detail'),
    },
    paid: {
      icon: 'check',
      chip: 'chip chip-ready',
      title: t('checkout.state.paid.title'),
      detail: t('checkout.state.paid.detail'),
    },
    expired: {
      icon: 'clock',
      chip: 'chip chip-due',
      title: t('checkout.state.expired.title'),
      detail: t('checkout.state.expired.detail'),
    },
    review: {
      icon: 'alert',
      chip: 'chip chip-due',
      title: t('checkout.state.review.title'),
      detail: t('checkout.state.review.detail'),
    },
    failed: {
      icon: 'close',
      chip: 'chip chip-failed',
      title: t('checkout.state.failed.title'),
      detail: t('checkout.state.failed.detail'),
    },
    offline: {
      icon: 'alert',
      chip: 'chip chip-failed',
      title: t('checkout.state.offline.title'),
      detail: t('checkout.state.offline.detail'),
    },
  }

  const current = shown[state]

  return (
    <div className="card flex flex-col gap-3 p-4" role="status" aria-live="polite">
      <div className="flex items-start gap-3">
        <Icon name={current.icon} className="mt-0.5 shrink-0 text-muted" />
        <div className="flex min-w-0 flex-col gap-1">
          <p className="text-lg font-medium">{current.title}</p>
          <span className={current.chip}>{t(`checkout.state.${state}.chip`)}</span>
          <p className="max-w-prose text-sm text-muted">{current.detail}</p>
        </div>
      </div>

      {/* The countdown is a separate line rather than part of the heading: it
          changes every second, and a live region that re-announces the whole state
          every second is unusable with a screen reader. */}
      {countdownLabel !== null && state === 'waiting' && (
        <p className="meta">{t('checkout.expiresIn', { time: countdownLabel })}</p>
      )}

      {state === 'paid' && (
        <Link to="/billing" className="link inline-flex min-h-11 items-center self-start">
          {t('checkout.viewPlan')}
        </Link>
      )}

      {state === 'expired' && onNewCode !== undefined && (
        <button type="button" className="btn btn-primary min-h-11 self-start" onClick={onNewCode}>
          {t('checkout.newCode')}
        </button>
      )}

      {state === 'offline' && onRetry !== undefined && (
        <button type="button" className="btn btn-secondary min-h-11 self-start" onClick={onRetry}>
          {t('checkout.retry')}
        </button>
      )}
    </div>
  )
}
