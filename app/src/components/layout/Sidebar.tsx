import { useRef, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router'
import {
  AnimatePresence,
  LayoutGroup,
  motion,
  useMotionValue,
  useReducedMotion,
} from 'motion/react'
import type { MotionValue } from 'motion/react'

import { Wordmark } from '@/components/ui/BrandIllustration'
import { Icon, type IconName } from '@/components/ui/Icon'
import { useWorkspaces } from '@/api/queries/workspaces'
import { useSession } from '@/features/auth/useSession'
import { DEFAULT_TIMEZONE } from '@/lib/time'
import { useFormFactor } from '@/lib/useFormFactor'
import { useDockMagnification } from './TabBar'

const STORAGE_KEY = 'stvdy.sidebar'
const EXPANDED_W = 216
const COLLAPSED_W = 64

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

const SPRING = { type: 'spring' as const, stiffness: 300, damping: 30 }
const INSTANT = { duration: 0 }

/**
 * The desktop sidebar.
 *
 * Two modes: **expanded** (216px, icon + label, courses, footer) and **collapsed**
 * (64px, icon-only with dock magnification on hover). The collapsed state is
 * persisted in localStorage so it survives a reload.
 *
 * The transition animates `width` and `padding` together on the `motion.nav`,
 * and content inside clips via `overflow: hidden`. Labels animate `maxWidth` +
 * `opacity` in place; courses and footer fade out then unmount via
 * `AnimatePresence`, so nothing pops during the spring.
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
  const spring = reducedMotion ? INSTANT : SPRING

  function toggle() {
    setCollapsed((prev) => {
      const next = !prev
      persistCollapsed(next)
      if (!next) pointerY.set(Infinity)
      return next
    })
  }

  return (
    <motion.nav
      aria-label={t('shell.nav.main')}
      className="sidebar hidden shrink-0 flex-col gap-8 overflow-clip pb-6 md:flex"
      style={{ paddingTop: 'calc(1.75rem + env(safe-area-inset-top, 0px))' }}
      animate={{
        width: collapsed ? COLLAPSED_W : EXPANDED_W,
        paddingLeft: collapsed ? 8 : 20,
        paddingRight: collapsed ? 8 : 20,
      }}
      transition={spring}
      onPointerMove={collapsed ? (e) => pointerY.set(e.clientY) : undefined}
      onPointerLeave={collapsed ? () => pointerY.set(Infinity) : undefined}
    >
      {/* Logo row */}
      <div className="flex w-full items-center" style={{ minHeight: 35 }}>
        <AnimatePresence mode="popLayout" initial={false}>
          {collapsed ? (
            <motion.button
              key="expand"
              type="button"
              onClick={toggle}
              aria-label={t('shell.expandSidebar')}
              className="sidebar-toggle mx-auto"
              initial={{ opacity: 0, scale: 0.8 }}
              animate={{ opacity: 1, scale: 1 }}
              exit={{ opacity: 0, scale: 0.8 }}
              transition={{ duration: 0.15 }}
            >
              <Icon name="chevron-right" size={20} />
            </motion.button>
          ) : (
            <motion.div
              key="logo"
              className="flex w-full items-center justify-between gap-2"
              initial={{ opacity: 0 }}
              animate={{ opacity: 1 }}
              exit={{ opacity: 0 }}
              transition={{ duration: 0.15 }}
            >
              <Wordmark />
              <button
                type="button"
                onClick={toggle}
                aria-label={t('shell.collapseSidebar')}
                className="sidebar-toggle shrink-0"
              >
                <Icon name="chevron-left" size={16} />
              </button>
            </motion.div>
          )}
        </AnimatePresence>
      </div>

      <LayoutGroup id="desktop-navigation">
        <div className="flex w-full flex-col gap-1">
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
              spring={spring}
            />
          ))}
        </div>
      </LayoutGroup>

      <AnimatePresence>
        {!collapsed && (
          <motion.div
            key="secondary"
            className="flex w-full flex-1 flex-col gap-8"
            initial={{ opacity: 0, y: 8 }}
            animate={{ opacity: 1, y: 0 }}
            exit={{ opacity: 0, y: 8 }}
            transition={{ ...spring, opacity: { duration: 0.2 } }}
          >
            <SemesterCourses />
            <SidebarFooter />
          </motion.div>
        )}
      </AnimatePresence>
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
  spring,
}: {
  to: string
  label: string
  icon: IconName
  end: boolean
  collapsed: boolean
  pointerY: MotionValue<number>
  disableMagnification: boolean
  reducedMotion: boolean | null
  spring: Record<string, unknown>
}) {
  const ref = useRef<HTMLAnchorElement>(null)
  const scale = useDockMagnification(pointerY, ref, disableMagnification, 'y')

  return (
    <NavLink
      ref={ref}
      to={to}
      end={end}
      className={'nav-item' + (collapsed ? ' sidebar-item-collapsed' : '')}
      aria-label={collapsed ? label : undefined}
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
            style={collapsed && !disableMagnification ? { scale } : undefined}
          >
            <Icon name={icon} className="nav-icon relative z-10 shrink-0" />
          </motion.span>
          <motion.span
            className="relative z-10 whitespace-nowrap overflow-hidden"
            animate={{
              opacity: collapsed ? 0 : 1,
              maxWidth: collapsed ? 0 : EXPANDED_W - 40,
            }}
            transition={spring}
          >
            {label}
          </motion.span>
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
