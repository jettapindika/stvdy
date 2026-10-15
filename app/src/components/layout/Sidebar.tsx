import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router'
import { LayoutGroup, motion, useMotionValue, useReducedMotion } from 'motion/react'
import type { MotionValue } from 'motion/react'

import { Wordmark } from '@/components/ui/BrandIllustration'
import { Icon, type IconName } from '@/components/ui/Icon'
import { useWorkspaces } from '@/api/queries/workspaces'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE } from '@/lib/time'
import { useFormFactor } from '@/lib/useFormFactor'
import { useDockMagnification } from './TabBar'

const STORAGE_KEY = 'stvdy.sidebar'

function readCollapsed(): boolean {
  try {
    return window.localStorage.getItem(STORAGE_KEY) === 'collapsed'
  } catch {
    return false
  }
}

function persistCollapsed(collapsed: boolean): void {
  try {
    window.localStorage.setItem(STORAGE_KEY, collapsed ? 'collapsed' : 'expanded')
  } catch {
    // Best-effort preference.
  }
}

/**
 * The desktop sidebar.
 *
 * Two modes: **expanded** (216px, icon + label, courses, footer) and **collapsed**
 * (64px, icon-only with dock magnification on hover). The collapsed state is
 * persisted in localStorage so it survives a reload.
 *
 * When collapsed, moving the pointer along the rail magnifies each icon with the
 * same spring physics the mobile dock uses — the `useDockMagnification` hook
 * drives a CSS `scale` transform on the Y axis. The effect is the macOS dock
 * behaviour turned vertical.
 */
export function Sidebar() {
  const { t } = useTranslation()
  const reducedMotion = useReducedMotion()
  const [collapsed, setCollapsed] = useState(readCollapsed)
  const pointerY = useMotionValue(Infinity)

  const items: { to: string; label: string; icon: IconName }[] = [
    { to: '/', label: t('nav.today'), icon: 'today' },
    { to: '/workspaces', label: t('nav.workspaces'), icon: 'workspaces' },
    { to: '/study', label: t('nav.study'), icon: 'study' },
    { to: '/planner', label: t('nav.planner'), icon: 'planner' },
    { to: '/settings', label: t('nav.settings'), icon: 'settings' },
  ]

  const disableMagnification = !collapsed || !!reducedMotion

  function toggle() {
    setCollapsed((prev) => {
      const next = !prev
      persistCollapsed(next)
      // Reset magnification when expanding so springs settle.
      if (!next) pointerY.set(Infinity)
      return next
    })
  }

  return (
    <motion.nav
      aria-label={t('shell.nav.main')}
      className={
        'sidebar hidden shrink-0 flex-col gap-8 pb-6 md:flex'
        + (collapsed
          ? ' sidebar-collapsed w-16 items-center px-2 pt-[calc(1.75rem+env(safe-area-inset-top,0px))]'
          : ' w-54 px-5 pt-[calc(1.75rem+env(safe-area-inset-top,0px))]')
      }
      onPointerMove={collapsed ? (e) => pointerY.set(e.clientY) : undefined}
      onPointerLeave={collapsed ? () => pointerY.set(Infinity) : undefined}
      animate={{ width: collapsed ? 64 : 216 }}
      transition={reducedMotion ? { duration: 0 } : { type: 'spring', stiffness: 300, damping: 30 }}
    >
      {/* Logo row: wordmark (expanded) or expand button (collapsed), with toggle. */}
      <div className={'flex w-full items-center' + (collapsed ? ' justify-center' : ' justify-between')}>
        {collapsed ? (
          <button
            type="button"
            onClick={toggle}
            aria-label={t('shell.expandSidebar')}
            className="sidebar-toggle"
          >
            <Icon name="chevron-right" size={20} />
          </button>
        ) : (
          <>
            <Wordmark />
            <button
              type="button"
              onClick={toggle}
              aria-label={t('shell.collapseSidebar')}
              className="sidebar-toggle"
            >
              <Icon name="chevron-left" size={16} />
            </button>
          </>
        )}
      </div>

      <LayoutGroup id="desktop-navigation">
        <div className={'flex w-full flex-col' + (collapsed ? ' items-center gap-1' : ' gap-2')}>
          {items.map((item) => (
            <SidebarItem
              key={item.to}
              to={item.to}
              label={item.label}
              icon={item.icon}
              end={item.to === '/'}
              collapsed={collapsed}
              pointerY={pointerY}
              disableMagnification={disableMagnification}
              reducedMotion={reducedMotion}
            />
          ))}
        </div>
      </LayoutGroup>

      {!collapsed && <SemesterCourses />}
      {!collapsed && <SidebarFooter />}
    </motion.nav>
  )
}

function SidebarItem({
  to,
  label,
  icon,
  end,
  collapsed,
  pointerY,
  disableMagnification,
  reducedMotion,
}: {
  to: string
  label: string
  icon: IconName
  end: boolean
  collapsed: boolean
  pointerY: MotionValue<number>
  disableMagnification: boolean
  reducedMotion: boolean | null
}) {
  const ref = useRef<HTMLAnchorElement>(null)
  const scale = useDockMagnification(pointerY, ref, disableMagnification, 'y')

  if (collapsed) {
    return (
      <NavLink
        ref={ref}
        to={to}
        end={end}
        className="nav-item sidebar-item-collapsed"
        aria-label={label}
      >
        {({ isActive }) => (
          <>
            {isActive && (
              <motion.span
                aria-hidden="true"
                className="nav-selection"
                layoutId="selected-page"
                transition={reducedMotion ? { duration: 0 } : { type: 'spring', stiffness: 380, damping: 34 }}
              />
            )}
            <motion.span
              className="dock-icon-sizer"
              style={disableMagnification ? undefined : { scale }}
            >
              <Icon name={icon} className="nav-icon relative z-10 shrink-0" />
            </motion.span>
          </>
        )}
      </NavLink>
    )
  }

  return (
    <NavLink ref={ref} to={to} end={end} className="nav-item">
      {({ isActive }) => (
        <>
          {isActive && (
            <motion.span
              aria-hidden="true"
              className="nav-selection"
              layoutId="selected-page"
              transition={reducedMotion ? { duration: 0 } : { type: 'spring', stiffness: 380, damping: 34 }}
            />
          )}
          <Icon name={icon} className="nav-icon relative z-10 shrink-0" />
          <span className="relative z-10">{label}</span>
        </>
      )}
    </NavLink>
  )
}

/**
 * The course list under the rail's divider.
 */
function SemesterCourses() {
  const { t } = useTranslation()
  const isPhone = useFormFactor() === 'phone'
  const workspaces = useWorkspaces({ enabled: !isPhone })

  const rows = workspaces.data ?? []
  if (rows.length === 0) return null

  return (
    <>
      <div className="h-px w-full bg-border" />
      <div className="flex w-full flex-col gap-5">
        <p className="eyebrow">{t('shell.semester')}</p>
        {rows.map((workspace) => (
          <NavLink
            key={workspace.id}
            to={`/workspaces/${workspace.id}`}
            className="flex w-full flex-col gap-1 rounded-field hover:bg-surface-raised"
          >
            {workspace.course_code && (
              <span className="meta text-primary-text">{workspace.course_code}</span>
            )}
            <span className="truncate text-label font-medium text-text" title={workspace.name}>
              {workspace.name}
            </span>
          </NavLink>
        ))}
      </div>
    </>
  )
}

/**
 * The rail's closing note.
 */
function SidebarFooter() {
  const { t } = useTranslation()
  const session = useSession()
  const zone = session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

  return (
    <div className="mt-auto flex flex-col gap-2">
      <Icon name="help" size={20} className="shrink-0" />
      <p className="text-caption text-muted">{t('shell.railNote')}</p>
      <p className="meta uppercase">{zone}</p>
    </div>
  )
}
