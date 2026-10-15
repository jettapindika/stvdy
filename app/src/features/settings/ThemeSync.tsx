import { useEffect } from 'react'

import { useSettings } from '@/api/queries/settings'
import { applyAppearance, isAppearanceTheme } from '@/lib/appearance'

/** Applies the signed-in account preference after the cached settings row refreshes. */
export function ThemeSync() {
  const { data, dataUpdatedAt } = useSettings()
  const savedTheme = data?.settings?.theme

  useEffect(() => {
    if (isAppearanceTheme(savedTheme)) applyAppearance(savedTheme)
  }, [savedTheme, dataUpdatedAt])

  return null
}
