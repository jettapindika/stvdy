import { NavLink } from 'react-router'

const links = [
  { to: '/', label: 'Today' },
  { to: '/workspaces', label: 'Workspaces' },
  { to: '/calendar', label: 'Calendar' },
  { to: '/tasks', label: 'Tasks' },
  { to: '/settings', label: 'Settings' },
]

export function Sidebar() {
  return (
    <nav
      aria-label="Main"
      className="hidden w-56 shrink-0 border-r border-gray-200 p-4 sm:block dark:border-neutral-800"
    >
      <ul className="space-y-1">
        {links.map((link) => (
          <li key={link.to}>
            {/* `.nav-link` carries the focus-visible ring and the active state
                (see styles.css), so both live in one place. */}
            <NavLink
              to={link.to}
              end={link.to === '/'}
              className={({ isActive }) =>
                `nav-link ${isActive ? 'text-brand-900' : 'text-gray-600 hover:bg-gray-50'}`
              }
            >
              {link.label}
            </NavLink>
          </li>
        ))}
      </ul>
    </nav>
  )
}
