import { NavLink } from 'react-router'

const tabs = [
  { to: '/', label: 'Today' },
  { to: '/workspaces', label: 'Work' },
  { to: '/calendar', label: 'Calendar' },
  { to: '/tasks', label: 'Tasks' },
  { to: '/settings', label: 'More' },
]

/**
 * Bottom navigation for phones. Each target is at least 44px tall, the minimum
 * comfortable touch target, and the bar clears the home indicator via
 * `pb-safe-bottom`. The focus-visible ring comes from `.nav-link`.
 */
export function TabBar() {
  return (
    <nav
      aria-label="Main"
      className="fixed inset-x-0 bottom-0 flex border-t border-gray-200 bg-white pb-safe-bottom sm:hidden dark:border-neutral-800 dark:bg-neutral-950"
    >
      {tabs.map((tab) => (
        <NavLink
          key={tab.to}
          to={tab.to}
          end={tab.to === '/'}
          className={({ isActive }) =>
            `nav-link flex min-h-11 flex-1 items-center justify-center text-xs ${
              isActive ? 'text-brand-900' : 'text-gray-500'
            }`
          }
        >
          {tab.label}
        </NavLink>
      ))}
    </nav>
  )
}
