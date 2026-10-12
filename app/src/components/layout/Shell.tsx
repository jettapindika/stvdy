import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'

import { useSyncEngine } from '@/features/sync/useSyncEngine'
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
 * `main` is a flex column (`.shell-main`) so a screen fills the space it is
 * *given*. A screen that sizes itself with `100dvh` instead ignores the tab bar
 * and its safe-area inset, which produces an outer page scroll around panels that
 * already scroll internally, plus a last row hidden behind the bar.
 *
 * **On a phone the shell root is a column** (`.shell-phone`) and the tab bar is a
 * flow sibling *below* `main`, not a fixed overlay above it. That is the fix for
 * the defect the Phase 8 gate found: a fixed band is painted over the content, so
 * the content has to reserve the band's height with a number in the stylesheet,
 * and the band's height is not a number — the sync chip above the five labels
 * measures 73px at 100% text and 176px at 200%, because `max-w-64` makes it wrap.
 * Any reserved constant is therefore wrong at some text scale. In flow the band
 * takes the height it needs and `main` takes the rest, so there is no arithmetic
 * to get wrong.
 *
 * The skip link is not optional (R-32): a keyboard user should not have to tab
 * through the whole nav to reach content, and `<main>` is the landmark to skip to.
 *
 * The shell also mounts the sync engine's glue (`useSyncEngine`), and that is the
 * right place for it: the shell only exists for a signed-in account, so the engine
 * is configured when there is a session and torn down when there is not. It is a
 * no-op on the web build, which has no local database and no outbox (A14).
 */
export function Shell({ children }: { children: ReactNode }) {
  const { t } = useTranslation()
  const isPhone = useFormFactor() === 'phone'
  useSyncEngine()

  return (
    <div className={`flex min-h-[100dvh] bg-bg text-text${isPhone ? ' shell-phone' : ''}`}>
      <a
        href="#main"
        className="btn btn-secondary sr-only focus:not-sr-only focus:absolute focus:left-3 focus:top-3 focus:z-50"
      >
        {t('shell.skipToContent')}
      </a>

      {!isPhone && <Sidebar />}

      <main
        id="main"
        className="shell-main pt-safe-top"
      >
        {children}
      </main>

      {isPhone && <TabBar />}
    </div>
  )
}
