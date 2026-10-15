import type { ReactNode } from 'react'
import { useTranslation } from 'react-i18next'
import { useMatch } from 'react-router'

import { CreditMeter } from '@/features/billing/CreditMeter'
import { ThemeSync } from '@/features/settings/ThemeSync'
import { SyncStatus } from '@/features/sync/SyncStatus'
import { useSyncEngine } from '@/features/sync/useSyncEngine'
import { useFormFactor } from '@/lib/useFormFactor'
import { Sidebar } from './Sidebar'
import { TabBar } from './TabBar'
import { PhoneHeader, TopBar } from './TopBar'

/**
 * The app shell.
 *
 * The shell is chosen by viewport width, not by platform: a phone browser and a
 * native iOS build want the same layout. `useFormFactor` owns that decision and
 * its breakpoints match the CSS ones (768px for the sidebar/tab-bar swap).
 *
 * **The desktop shell gained a top bar in the redesign.** It is `Sidebar | (TopBar
 * over main)` on a desktop and `(PhoneHeader over main) | TabBar` on a phone, which
 * is what every frame in the design draws. The two form factors have different
 * chrome because they have different room: a phone header holds a wordmark, one
 * icon and an avatar, and the credit meter drops to its own row under them.
 *
 * `main` is a flex column (`.shell-main`) so a screen fills the space it is
 * *given*. A screen that sizes itself with `100dvh` instead ignores the tab bar
 * and its safe-area inset, which produces an outer page scroll around panels that
 * already scroll internally, plus a last row hidden behind the bar.
 *
 * **On a phone the shell root is a column** (`.shell-phone`) and the tab bar is a
 * flow sibling *below* `main`, not a fixed overlay above it. The bar now contains
 * only the five navigation links and its own safe-area inset, so its height is
 * stable and content is never painted beneath a second status row. The Today page
 * owns its credit meter in content, matching the mobile frame's hierarchy.
 *
 * The skip link is not optional (R-32): a keyboard user should not have to tab
 * through the whole nav to reach content, and `<main>` is the landmark to skip to.
 *
 * The shell also mounts the sync engine's glue (`useSyncEngine`), and that is the
 * right place for it: the shell only exists for a signed-in account, so the engine
 * is configured when there is a session and torn down when there is not. It is a
 * no-op on the web build, which has no local database and no outbox (A14).
 *
 * **The drag strip is present but inert, and that is deliberate.** `src-tauri/
 * tauri.conf.json` sets no `decorations: false`, so the window is decorated today
 * and the OS title bar is what moves it. `.drag-region` is therefore zero-height in
 * every current build, and the top bar does not reserve space for it. The rule and
 * the class are in place so that turning on a frameless window is a config change
 * plus adding `window-frameless` to this root — not a redesign of the bar. Until
 * then, claiming a drag region would be reserving 36px for chrome that is not
 * there.
 */
export function Shell({
  children,
  onOpenPalette,
}: {
  children: ReactNode
  onOpenPalette: () => void
}) {
  const { t } = useTranslation()
  const isPhone = useFormFactor() === 'phone'
  const focusedSession = useMatch('/study/:deckId') !== null
  const workspaceDirectory = useMatch('/workspaces') !== null
  useSyncEngine()

  return (
    <div
      className={`flex min-h-[100dvh] bg-bg text-text${isPhone ? ' shell-phone' : ''}`}
    >
      <ThemeSync />
      <a
        href="#main"
        className="btn btn-secondary sr-only focus:not-sr-only focus:absolute focus:left-3 focus:top-3 focus:z-50"
      >
        {t('shell.skipToContent')}
      </a>

      {!isPhone && !focusedSession && <Sidebar />}

      {/* The content column. On a desktop the top bar is inside it, so the bar
          starts where the sidebar ends rather than spanning the window — which is
          what the design draws, and what keeps the sidebar's own wordmark at the
          window's top-left corner. */}
      <div className="flex min-w-0 flex-1 flex-col">
        {!focusedSession && (isPhone ? <PhoneHeader onOpenPalette={onOpenPalette} /> : <TopBar onOpenPalette={onOpenPalette} />)}
        {isPhone && !focusedSession && <SyncStatus variant="notice" />}
        {isPhone && workspaceDirectory && (
          <div className="px-5 pt-5"><CreditMeter /></div>
        )}

        <main id="main" tabIndex={-1} className="shell-main">
          {children}
        </main>
      </div>

      {isPhone && !focusedSession && <TabBar />}
    </div>
  )
}
