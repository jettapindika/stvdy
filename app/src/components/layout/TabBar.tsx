import { useRef } from 'react'
import {
  LayoutGroup,
  motion,
  useMotionValue,
  useReducedMotion,
  useSpring,
  useTransform,
  type MotionValue,
} from 'motion/react'
import { useTranslation } from 'react-i18next'
import { NavLink } from 'react-router'

import { Icon, type IconName } from '@/components/ui/Icon'

/** Base icon size in the dock. The viewBox is 24×24; this is the rendered px. */
const ICON_SIZE = 22
/** The scale factor at the pointer's center: 1.55× ≈ 34px from 22px. */
const MAX_SCALE = 1.55
/** Pixel radius of the magnification field around the pointer. */
const DISTANCE = 120

/**
 * A spring-driven scale value that grows as the pointer approaches.
 *
 * The hook is shared between the mobile dock and the collapsed sidebar so the
 * physics are identical in both places. It returns a `MotionValue<number>`
 * representing a CSS scale factor (1 = resting, MAX_SCALE = fully magnified).
 */
export function useDockMagnification(
  pointerPos: MotionValue<number>,
  ref: React.RefObject<HTMLElement | null>,
  disabled: boolean,
  axis: 'x' | 'y' = 'x',
) {
  const dist = useTransform(pointerPos, (pos: number) => {
    const bounds = ref.current?.getBoundingClientRect()
    if (!bounds) return Infinity
    const center = axis === 'x'
      ? bounds.x + bounds.width / 2
      : bounds.y + bounds.height / 2
    return pos - center
  })

  const raw = useTransform(
    dist,
    [-DISTANCE, 0, DISTANCE],
    [1, disabled ? 1 : MAX_SCALE, 1],
  )

  return useSpring(raw, { mass: 0.1, stiffness: 170, damping: 14 })
}

function DockItem({
  to,
  label,
  icon,
  end,
  pointerX,
  disableMagnification,
  reducedMotion,
}: {
  to: string
  label: string
  icon: IconName
  end: boolean
  pointerX: MotionValue<number>
  disableMagnification: boolean
  reducedMotion: boolean | null
}) {
  const ref = useRef<HTMLAnchorElement>(null)
  const scale = useDockMagnification(pointerX, ref, disableMagnification, 'x')

  return (
    <NavLink ref={ref} key={to} to={to} end={end} className="nav-item nav-item-tab">
      {({ isActive }) => (
        <>
          {isActive && (
            <motion.span
              aria-hidden="true"
              className="nav-selection"
              layoutId="selected-tab"
              transition={reducedMotion ? { duration: 0 } : { type: 'spring', stiffness: 380, damping: 34 }}
            />
          )}
          <motion.span
            aria-hidden="true"
            className="dock-icon-sizer"
            style={disableMagnification ? undefined : { scale }}
          >
            <Icon name={icon} size={ICON_SIZE} className="nav-icon relative z-10 shrink-0" />
          </motion.span>
          <span className="nav-label relative z-10">{label}</span>
        </>
      )}
    </NavLink>
  )
}

export function TabBar() {
  const { t } = useTranslation()
  const reducedMotion = useReducedMotion()
  const pointerX = useMotionValue(Infinity)
  const items: { to: string; label: string; icon: IconName }[] = [
    { to: '/', label: t('nav.today'), icon: 'today' },
    { to: '/workspaces', label: t('nav.work'), icon: 'workspaces' },
    { to: '/study', label: t('nav.study'), icon: 'study' },
    { to: '/planner', label: t('nav.planner'), icon: 'planner' },
    { to: '/settings', label: t('nav.more'), icon: 'ellipsis' },
  ]

  const disableMagnification = !!reducedMotion

  return (
    <div
      className="mobile-dock md:hidden"
      onPointerMove={(e) => pointerX.set(e.clientX)}
      onPointerLeave={() => pointerX.set(Infinity)}
      onTouchMove={(e) => {
        const touch = e.touches[0]
        if (touch) pointerX.set(touch.clientX)
      }}
      onTouchEnd={() => pointerX.set(Infinity)}
    >
      <LayoutGroup id="mobile-navigation">
        <nav aria-label={t('shell.nav.main')} className="dock-nav">
          {items.map((item) => (
            <DockItem
              key={item.to}
              to={item.to}
              label={item.label}
              icon={item.icon}
              end={item.to === '/'}
              pointerX={pointerX}
              disableMagnification={disableMagnification}
              reducedMotion={reducedMotion}
            />
          ))}
        </nav>
      </LayoutGroup>
    </div>
  )
}
