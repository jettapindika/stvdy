import { NavLink } from 'react-router'

import { NAV_ITEMS } from './Sidebar'

/**
 * Bottom navigation for phones. Each target is at least 44px tall (R-03), and
 * the bar clears the home indicator via `pb-safe-bottom`.
 *
 * The active item is marked by weight and an ink rule rather than by the accent,
 * because the accent is rationed to the provenance layer (DESIGN.md).
 */
export function TabBar() {
  return (
    <nav
      aria-label="Main"
      className="fixed inset-x-0 bottom-0 z-40 flex border-t border-rule bg-paper pb-safe-bottom md:hidden"
    >
      {NAV_ITEMS.map((item) => (
        <NavLink
          key={item.to}
          to={item.to}
          end={item.to === '/'}
          className="flex min-h-11 flex-1 items-center justify-center border-t-2 border-transparent px-1 text-xs text-ink-muted aria-[current=page]:border-ink aria-[current=page]:font-medium aria-[current=page]:text-ink"
        >
          {item.short}
        </NavLink>
      ))}
    </nav>
  )
}
