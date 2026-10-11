import type { ReactNode } from 'react'

import { platform } from '@/platform'
import { Sidebar } from './Sidebar'
import { TabBar } from './TabBar'

/**
 * Adaptive shell: sidebar on desktop and tablet, bottom tab bar on phones. The
 * platform adapter decides which, so no Tauri import appears here.
 *
 * The phone tab bar is fixed, so the main area reserves space for it with
 * bottom padding rather than the content hiding underneath.
 */
export function Shell({ children }: { children: ReactNode }) {
  const isPhone = platform.formFactor() === 'phone'

  return (
    <div className="flex min-h-[100dvh] bg-white text-gray-900 dark:bg-neutral-950 dark:text-neutral-100">
      {!isPhone && <Sidebar />}
      <main className={`flex-1 pt-safe-top ${isPhone ? 'pb-20' : ''}`}>{children}</main>
      {isPhone && <TabBar />}
    </div>
  )
}
