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
export const NAV_ITEMS = [
  { to: '/', label: 'Today', short: 'Today' },
  { to: '/workspaces', label: 'Workspaces', short: 'Work' },
  { to: '/calendar', label: 'Calendar', short: 'Calendar' },
  { to: '/tasks', label: 'Tasks', short: 'Tasks' },
  { to: '/settings', label: 'Settings', short: 'More' },
] as const

export function Sidebar() {
  return (
    <nav
      aria-label="Main"
      className="hidden w-60 shrink-0 flex-col gap-1 border-r border-rule bg-card px-3 py-4 pt-safe-top md:flex"
    >
      <Wordmark className="mb-4 px-2" />

      {NAV_ITEMS.map((item) => (
        <NavLink key={item.to} to={item.to} end={item.to === '/'} className="nav-item">
          {item.label}
        </NavLink>
      ))}
    </nav>
  )
}
