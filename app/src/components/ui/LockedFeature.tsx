import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { Icon } from '@/components/ui/Icon'
import { lockedFeaturePolicy } from '@/features/billing/policy'
import { platform } from '@/platform'

/**
 * A feature the user's plan does not include.
 *
 * The copy differs by platform, and that difference is a store rule rather than a
 * design choice (spec § UX rules, A24):
 *
 * - **web and desktop** may say which plan has it and offer a button to go and
 *   buy it, because QRIS checkout lives there.
 * - **iOS and Android** may not: no price, no upgrade button, no link or
 *   instruction pointing at a website purchase. The user is told the honest fact
 *   ("Not available on your plan") and nothing else.
 *
 * **The feature name arrives as a translation key, not as a string.** An earlier
 * version took `what="Rebalance my week"`, which every call site would have filled
 * with English — so an Indonesian user would see a translated frame around an
 * untranslated feature name, and no test could catch it because the string would
 * be a literal in a component. `whatKey` cannot drift: the i18n contract test
 * scans `*Key="…"` props and fails when the key is missing from `en.json`, and
 * `t()` inside this component resolves it in the user's language.
 *
 * The platform comes from the `platform` adapter, not from a user agent, so a
 * native build reports its own kind rather than whatever the webview claims.
 */
export function LockedFeature({
  /** The feature's name, as a key in the locale files. */
  whatKey,
  /** The plan that includes it, for the web/desktop variant. */
  minPlan,
  /** A one-line reason it is useful, as a key. Shown on every platform. */
  reasonKey,
}: {
  whatKey: string
  minPlan: 'Pro' | 'Power'
  reasonKey?: string
}) {
  const { t } = useTranslation()
  const policy = lockedFeaturePolicy(platform.kind())

  // `min-w-0` on the root: this card sits in a wrapping row (`PlannerScreen`'s action
  // row) and a flex item's automatic minimum size is its content, so at 200% text the
  // card forced its container 28px wider than the column instead of shrinking into it.
  // The `min-w-0` on the body below was not enough — the constraint has to be on the
  // item the parent measures.
  return (
    <div className="card flex min-w-0 items-start gap-3 p-4">
      <Icon name="lock" className="mt-0.5 shrink-0 text-subtle" />
      <div className="flex min-w-0 flex-col gap-1">
        <p className="text-small font-medium text-text break-words">{t(whatKey)}</p>
        {reasonKey === undefined ? null : (
          <p className="text-small text-muted break-words">{t(reasonKey)}</p>
        )}
        <p className="text-small text-subtle break-words">
          {policy.showUpgradeCta ? t('lock.availableOn', { plan: minPlan }) : t('lock.notOnPlan')}
        </p>
        {/* The one control on this card, and it is the control a user who has
            just been told the feature is locked is meant to reach. A bare `link`
            is text-height — measured in Chromium at 390px it was **93x19**,
            against the 44px phone target the accessibility gate requires — so it
            carries `min-h-11` and centres its label the way `States.tsx`'s plan
            link does. `self-start` keeps it as wide as its label rather than
            stretching to the card. */}
        {policy.showUpgradeCta ? (
          <Link
            to="/billing"
            className="link mt-1 inline-flex min-h-11 items-center self-start text-small"
          >
            {t('lock.upgrade', { plan: minPlan })}
          </Link>
        ) : null}
      </div>
    </div>
  )
}
