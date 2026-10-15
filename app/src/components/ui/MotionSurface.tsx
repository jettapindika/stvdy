import type { ReactNode } from 'react'
import { motion, useReducedMotion } from 'motion/react'

const entrance = { duration: 0.3, ease: [0.16, 1, 0.3, 1] as const }

export function MotionSurface({
  children,
  className,
}: {
  children: ReactNode
  className?: string
}) {
  const reducedMotion = useReducedMotion()

  return (
    <motion.div
      data-page-transition
      className={className}
      initial={reducedMotion ? false : { opacity: 0, y: 10 }}
      animate={{ opacity: 1, y: 0 }}
      transition={reducedMotion ? { duration: 0 } : entrance}
    >
      {children}
    </motion.div>
  )
}
