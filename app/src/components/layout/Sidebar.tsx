import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router'

import { Wordmark } from '@/components/ui/BrandIllustration'

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
    </nav>
  )
}
