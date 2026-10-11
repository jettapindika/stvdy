import { NavLink } from 'react-router'

import { NAV_ITEMS } from './Sidebar'

/**
 * Bottom navigation for phones.
 *
 * Each item extends the shared `.nav-item` treatment via `.nav-item-tab` rather
 * than restating it, so the two navs cannot drift apart. The active marker is a
 * top rule instead of a left bar, because this bar sits at the bottom edge and
 * the marker points into the content from the edge it is on.
 *
 * `min-height: 2.75rem` from the base rule gives the 44px touch target (R-03),
 * and `pb-safe-bottom` clears the home indicator.
 */
export function TabBar() {
  return (
    <nav
      aria-label="Main"
      className="fixed inset-x-0 bottom-0 z-40 flex border-t border-rule bg-card pb-safe-bottom md:hidden"
    >
      {NAV_ITEMS.map((item) => (
        <NavLink
          key={item.to}
          to={item.to}
          end={item.to === '/'}
          className="nav-item nav-item-tab"
        >
          {item.short}
        </NavLink>
      ))}
    </nav>
  )
}
