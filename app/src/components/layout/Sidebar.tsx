import { NavLink } from 'react-router'

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
      <div className="mb-4 flex items-center gap-2 px-2">
        {/* The wordmark, as in the design: a small book mark plus the name. No
            invented logo: the mark is the book from the design's own
            illustration, which is a user-provided asset. */}
        <svg viewBox="0 0 24 24" className="h-5 w-5" aria-hidden="true" fill="none">
          <path
            d="M12 6C9.8 4.4 6.8 4 4.5 4.7v12.6c2.3-.7 5.3-.3 7.5 1.3"
            stroke="var(--accent)"
            strokeWidth="1.9"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
          <path
            d="M12 6c2.2-1.6 5.2-2 7.5-1.3v12.6c-2.3-.7-5.3-.3-7.5 1.3"
            stroke="var(--accent)"
            strokeWidth="1.9"
            strokeLinecap="round"
            strokeLinejoin="round"
          />
        </svg>
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
