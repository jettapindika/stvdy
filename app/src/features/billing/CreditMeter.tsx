import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { Icon } from '@/components/ui/Icon'
import { useUsage } from '@/api/queries/usage'
import { INTL_LOCALE, isLanguage } from '@/i18n'

/**
 * The credit balance, in the one place it is formatted.
 *
 * The number is the ledger's sum as `GET /usage` computes it — never a client
 * tally. A client that spent credits and did not hear back would show a balance
 * the server does not have, which is the honesty rule (roadmap gate 7b) applied
 * to the one number a student checks before asking a question.
 *
 * This module owns the *formatting* as well as the fetch, and that is the point
 * of it being one component: two meters with two `Intl.NumberFormat`s would
 * disagree about `1.500`, which is one and a half in Jakarta and fifteen hundred
 * in London. `formatCount` is exported so the plan screen's buckets and limit
 * rows use the same formatter rather than reaching for a second one.
 *
 * It is a link, not a label: the balance is the reason a student goes looking for
 * the plan screen, and a control is what makes it reachable by keyboard. It
 * carries no colour of its own — the state is in the words ("No credits left"),
 * so nothing is conveyed by colour alone.
 *
 * The capsule is shared by the desktop top bar and the mobile content summary.
 * At enlarged text sizes its label wraps rather than hiding any part of a balance.
 */

/**
 * A count, formatted for the language the reader is in.
 *
 * `id-ID` groups with a full stop (`1.500`) where `en-GB` groups with a comma
 * (`1,500`), and the two readings of the same string are the reason this is one
 * function rather than an `Intl.NumberFormat` per call site.
 */
export function formatCount(value: number, language: string): string {
  const tag = isLanguage(language) ? INTL_LOCALE[language] : INTL_LOCALE.en
  return new Intl.NumberFormat(tag).format(value)
}

export function CreditMeter({ className = '' }: { className?: string }) {
  const { t, i18n } = useTranslation()
  const usage = useUsage()
  // `credits` is reached with optional chaining even though the contract types it
  // as present. This component renders inside the app shell on *every* screen, so
  // a body that is not the documented envelope has to degrade to "unavailable"
  // rather than take the whole shell down with it.
  const total = usage.data?.credits?.total
  // The plan's monthly allowance and the unspent part of it, which is the pair the
  // level bar draws. Both are the payload's own numbers; a percentage is not, and
  // `limits.credits_per_month.used` is `null` by design, so the spent count is
  // deliberately not derived here either (the same reasoning `PlanUsageSections`
  // records for its card).
  const allowance = usage.data?.limits?.credits_per_month?.limit
  const remaining = usage.data?.credits?.monthly
  const share =
    typeof allowance === 'number' && allowance > 0 && typeof remaining === 'number'
      ? Math.min(remaining / allowance, 1) * 100
      : null

  const label = usage.isPending
    ? t('meter.loading')
    : total === undefined
      ? // The request failed, has not answered, or answered something that is
        // not the contract. No number is shown, because a balance this component
        // does not have is worse than none: the plan screen carries the failure
        // and its remedy.
        t('meter.unavailable')
      : total === 0
        ? t('meter.none')
        : t('meter.total', { credits: formatCount(total, i18n.language) })

  // The bar's own name, which is **not** the pill's label and cannot be derived from it:
  // the label states `credits.total` (the monthly bucket *plus* anything purchased, "72
  // credits left") while the bar draws `credits.monthly` against the plan's monthly
  // allowance. Those are different facts, so the bar needs a name of its own — the
  // `DashboardScreen` bar's `role="img"` + `aria-label` pattern rather than the plan
  // screen's `aria-hidden`, which is correct only because the two counts are printed
  // beside it there.
  const barLabel =
    typeof allowance === 'number' && typeof remaining === 'number'
      ? t('meter.level', {
          remaining: formatCount(remaining, i18n.language),
          allowance: formatCount(allowance, i18n.language),
        })
      : undefined

  return (
    <Link
      to="/billing"
      className={`credit-pill hover:border-border-strong focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring ${className}`}
    >
      <Icon name="sparkles" size={16} className="shrink-0 text-primary-text" />
      <span className="meta min-w-0 wrap-anywhere text-text">{label}</span>
      {/* The frame's 24x4 level bar, omitted rather than zero-filled when either number
          is absent — `Shell.test.tsx` answers `/usage` with `limits: {}`, and a bar drawn
          from an undefined allowance would be a figure nothing measured. */}
      {share !== null && barLabel !== undefined ? (
        <span className="credit-level" role="img" aria-label={barLabel}>
          <span className="credit-level-fill" style={{ width: `${String(share)}%` }} />
        </span>
      ) : null}
    </Link>
  )
}
