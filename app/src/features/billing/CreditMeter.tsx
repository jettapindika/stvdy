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
 * **Two variants, one reading.** `pill` is the shell's, in both form factors: the
 * desktop top bar's account group and the phone header's own row under it, which is
 * where the frames draw a bordered capsule beside the avatar. `plain` remains for a
 * caller that wants the bare label. Both render the same string from the same
 * formatter, so the number cannot differ between them.
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

export function CreditMeter({
  className = '',
  variant = 'plain',
}: {
  className?: string
  variant?: 'plain' | 'pill'
}) {
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

  if (variant === 'pill') {
    return (
      <Link
        to="/billing"
        className={`credit-pill hover:border-border-strong focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring ${className}`}
      >
        <Icon name="sparkles" size={16} className="shrink-0 text-primary-text" />
        {/* `whitespace-nowrap`: the balance is two words at most in either
            language, and a capsule that wrapped "60 credits left" onto two lines
            made the whole top bar twice its height. */}
        <span className="meta whitespace-nowrap text-text">{label}</span>
        {/* The frame's 24x4 level bar: a second reading of the same balance, never
            the only one — the label above says the count in words, so nothing here
            depends on the bar. `aria-hidden` for the same reason the plan screen's
            monthly bar is: the sentence is the fact, and a bar carrying a fact no
            text carries would be unreadable to a screen reader.
            The ratio is the payload's own two numbers (`limits.credits_per_month`
            is the plan's monthly allowance, `credits.monthly` the unspent part of
            it), which is the same pair `PlanUsageSections` draws — deliberately not
            a percentage, which is a figure the server never computes. Omitted, not
            zero-filled, when either number is absent. */}
        {share !== null ? (
          <span className="credit-level" aria-hidden="true">
            <span className="credit-level-fill" style={{ width: `${String(share)}%` }} />
          </span>
        ) : null}
      </Link>
    )
  }

  return (
    <Link
      to="/billing"
      className={`flex min-h-11 flex-col justify-center rounded-field px-2 py-1 hover:bg-surface-raised focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring ${className}`}
    >
      <span className="meta">{label}</span>
    </Link>
  )
}
