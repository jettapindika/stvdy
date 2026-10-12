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
    <div className="flex min-h-[100dvh] bg-bg text-text">
      <a
        href="#main"
        className="btn btn-secondary sr-only focus:not-sr-only focus:absolute focus:left-3 focus:top-3 focus:z-50"
      >
        {t('shell.skipToContent')}
      </a>

      {!isPhone && <Sidebar />}

      <main
        id="main"
        className={`shell-main pt-safe-top ${isPhone ? 'shell-main-phone' : ''}`}
      >
        {children}
      </main>

      {isPhone && <TabBar />}
    </div>
  )
}
