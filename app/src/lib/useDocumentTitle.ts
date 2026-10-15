import { useEffect } from 'react'
import { useTranslation } from 'react-i18next'
import { useLocation } from 'react-router'

/**
 * Sets `document.title` to match the current route (WCAG 2.4.2, Level A).
 *
 * The title is looked up from `routes.*` in the locale bundle, so it is
 * translated alongside everything else. A route that doesn't match any
 * known prefix falls back to the bare app name; a workspace route
 * interpolates the workspace name when one is available.
 *
 * Mounted once in the root route element so every navigation — including
 * auth screens and deep links — updates the title without each screen
 * remembering to.
 */
export function useDocumentTitle() {
  const { t } = useTranslation()
  const { pathname } = useLocation()

  useEffect(() => {
    const title = titleForPath(pathname, t)
    document.title = title
  }, [pathname, t])
}

/**
 * Map a pathname to its `routes.*` locale key. The order is longest-prefix
 * first, so `/settings/account` matches `settings` before the catch-all.
 */
function titleForPath(
  pathname: string,
  t: (key: string, opts?: Record<string, string>) => string,
): string {
  if (pathname === '/' || pathname === '/today') return t('routes.today')
  if (pathname.startsWith('/workspaces/')) return t('routes.workspace', { name: 'Workspace' })
  if (pathname === '/workspaces') return t('routes.workspaces')
  if (pathname.startsWith('/study/')) return t('routes.studyDeck')
  if (pathname === '/study') return t('routes.study')
  if (pathname.startsWith('/quiz/')) return t('routes.quiz')
  if (pathname.startsWith('/planner')) return t('routes.planner')
  if (pathname.startsWith('/settings')) return t('routes.settings')
  if (pathname.startsWith('/billing')) return t('routes.billing')
  if (pathname === '/devices') return t('routes.devices')
  if (pathname === '/admin') return t('routes.admin')
  if (pathname === '/dashboard') return t('routes.dashboard')
  if (pathname === '/signin') return t('routes.signIn')
  if (pathname === '/signup') return t('routes.signUp')
  if (pathname.startsWith('/verify')) return t('routes.verify')
  return 'Stvdy'
}
