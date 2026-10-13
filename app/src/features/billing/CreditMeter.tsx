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
 * **Two variants, one reading.** `pill` is the top bar's, where the balance sits
 * in a bordered capsule beside the avatar and has to stay on one line; `plain` is
 * the phone header's row, where there is width to spare. Both render the same
 * string from the same formatter, so the number cannot differ between them.
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
