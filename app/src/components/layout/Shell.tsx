import type { ReactNode } from 'react'

import { useFormFactor } from '@/lib/useFormFactor'
import { Sidebar } from './Sidebar'
import { TabBar } from './TabBar'

/**
 * The app shell.
 *
 * The shell is chosen by viewport width, not by platform: a phone browser and a
 * native iOS build want the same layout. `useFormFactor` owns that decision and
 * its breakpoints match the CSS ones (768px for the sidebar/tab-bar swap).
 *
 * Composition differs by form factor on purpose (RHYTHM 2): tablet and desktop
 * get a persistent sidebar, phones get a bottom tab bar. That is a real
 * difference in how the screen is used, not decoration.
 *
 * The skip link is not optional (R-32): a keyboard user should not have to tab
 * through the whole nav to reach content, and `<main>` is the landmark to skip to.
 */
export function Shell({ children }: { children: ReactNode }) {
  const isPhone = useFormFactor() === 'phone'

  return (
    <div className="flex min-h-[100dvh] bg-paper text-ink">
      <a
        href="#main"
        className="btn btn-secondary sr-only focus:not-sr-only focus:absolute focus:left-3 focus:top-3 focus:z-50"
      >
        Skip to content
      </a>

      {!isPhone && <Sidebar />}

      {/* Bottom padding on phones reserves room for the fixed tab bar, so the
          last row of content is never hidden behind it. */}
      <main id="main" className={`min-w-0 flex-1 pt-safe-top ${isPhone ? 'pb-20' : ''}`}>
        {children}
      </main>

      {isPhone && <TabBar />}
    </div>
  )
}
