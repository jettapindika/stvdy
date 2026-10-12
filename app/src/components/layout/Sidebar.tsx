import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router'

import { Wordmark } from '@/components/ui/BrandIllustration'
import { CreditMeter } from '@/features/billing/CreditMeter'
import { SyncStatus } from '@/features/sync/SyncStatus'

/**
 * Primary navigation, shared by the sidebar and the phone tab bar.
 *
 * No icons, deliberately. The five labels are short and unambiguous, and pulling
 * in a thin-stroke icon set would give the app the same recognizable look as
 * every other generated interface, which is the tell R-04 warns about. If a
 * glyph is ever added it should be drawn for this product, not imported.
 *
 * The two navs share `.nav-item` in styles.css (the tab bar extends it via
 * `.nav-item-tab`), so a change to the base treatment reaches both. That is what
 * keeps them from drifting apart.
 *
 * The credit meter sits below the nav rather than at the top with the wordmark:
 * it is the one number on this rail, and the bottom of the rail is where a
 * balance is looked for. It is a link to the plan screen, so the number and the
 * explanation of it are one `Tab` apart (R-32).
 */
export function Sidebar() {
  const { t } = useTranslation()
  // The labels are resolved here rather than at module scope: a constant holding
  // copy is evaluated before there is a language, so it could only ever ship one
  // of them.
  const items = [
    { to: '/', label: t('nav.today') },
    { to: '/workspaces', label: t('nav.workspaces') },
    { to: '/calendar', label: t('nav.calendar') },
    { to: '/tasks', label: t('nav.tasks') },
    { to: '/settings', label: t('nav.settings') },
  ]

  return (
    <nav
      aria-label={t('shell.nav.main')}
      className="hidden w-60 shrink-0 flex-col gap-1 border-r border-border bg-surface px-3 py-4 pt-safe-top md:flex"
    >
      <Wordmark className="mb-4 px-2" />

      {items.map((item) => (
        <NavLink key={item.to} to={item.to} end={item.to === '/'} className="nav-item">
          {item.label}
        </NavLink>
      ))}

      <div className="mt-auto flex flex-col gap-1 pt-4">
        <CreditMeter />
        {/* The status control sits at the foot of the rail, beside the meter, and
            it is the only route to /devices (A189): Devices is not one of the five
            labels this rail and the phone tab bar share, and a sixth would take
            width from all of them. */}
        <SyncStatus />
      </div>
    </nav>
  )
}
