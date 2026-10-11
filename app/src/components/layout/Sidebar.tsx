import { NavLink } from 'react-router'

/**
 * Primary navigation, shared by the sidebar and the phone tab bar so the two
 * cannot drift apart.
 *
 * No icons, deliberately. The five labels are short and unambiguous, and pulling
 * in a thin-stroke icon set would give the app the same recognizable look as
 * every other generated interface, which is the tell R-04 warns about. If a
 * glyph is ever added, it should be one drawn for this product, not imported.
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
      className="hidden w-60 shrink-0 flex-col gap-1 border-r border-rule bg-paper px-3 py-4 pt-safe-top md:flex"
    >
      <div className="mb-4 px-2">
        {/* Wordmark only. No invented logo mark: fabricating a brand asset
            without instruction is exactly what R-23 forbids. */}
        <span className="text-base font-semibold tracking-tight">Stvdy</span>
      </div>

      {NAV_ITEMS.map((item) => (
        <NavLink key={item.to} to={item.to} end={item.to === '/'} className="nav-item">
          {item.label}
        </NavLink>
      ))}
    </nav>
  )
}
