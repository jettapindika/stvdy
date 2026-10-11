import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router'

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
 *
 * The short labels differ from the sidebar's long ones, which is why they are
 * listed here rather than derived from it. They are resolved inside the component
 * because copy cannot live at module scope, where there is no language yet.
 */
export function TabBar() {
  const { t } = useTranslation()
  const items = [
    { to: '/', short: t('nav.today') },
    { to: '/workspaces', short: t('nav.work') },
    { to: '/calendar', short: t('nav.calendar') },
    { to: '/tasks', short: t('nav.tasks') },
    { to: '/settings', short: t('nav.more') },
  ]

  return (
    <nav
      aria-label={t('shell.nav.main')}
      className="fixed inset-x-0 bottom-0 z-40 flex border-t border-border bg-surface pb-safe-bottom md:hidden"
    >
      {items.map((item) => (
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
