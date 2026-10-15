import { COLORS, COURSES } from '@/tokens/tokens'

export type AppearanceTheme = 'system' | 'light' | 'dark'

const STORAGE_KEY = 'stvdy.theme'
const EXPLICIT_THEMES = ['light', 'dark'] as const

type ExplicitTheme = (typeof EXPLICIT_THEMES)[number]

/** Per-theme `--select-chevron` value: the stroke matches `--text-muted`. */
const SELECT_CHEVRON: Record<ExplicitTheme, string> = {
  light:
    "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16' fill='none'%3E%3Cpath d='M4 6l4 4 4-4' stroke='%234f4f5b' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E\")",
  dark:
    "url(\"data:image/svg+xml,%3Csvg xmlns='http://www.w3.org/2000/svg' width='16' height='16' fill='none'%3E%3Cpath d='M4 6l4 4 4-4' stroke='%23c2c2cc' stroke-width='1.5' stroke-linecap='round' stroke-linejoin='round'/%3E%3C/svg%3E\")",
}

export function isAppearanceTheme(value: unknown): value is AppearanceTheme {
  return value === 'system' || value === 'light' || value === 'dark'
}

function readStoredTheme(): AppearanceTheme | null {
  try {
    if (typeof window === 'undefined') return null
    const value = window.localStorage.getItem(STORAGE_KEY)
    return isAppearanceTheme(value) ? value : null
  } catch {
    return null
  }
}

function persistTheme(theme: AppearanceTheme): void {
  try {
    if (typeof window !== 'undefined') window.localStorage.setItem(STORAGE_KEY, theme)
  } catch {
    // Storage is a best-effort preference, not a boot requirement.
  }
}

function removeExplicitOverrides(root: HTMLElement): void {
  for (const theme of EXPLICIT_THEMES) {
    for (const name of Object.keys(COLORS[theme])) root.style.removeProperty(`--${name}`)
    for (const name of Object.keys(COURSES[theme])) {
      root.style.removeProperty(`--course-${name}`)
      root.style.removeProperty(`--course-${name}-tint`)
    }
  }
  root.style.removeProperty('--select-chevron')
}

function applyExplicitTheme(root: HTMLElement, theme: ExplicitTheme): void {
  for (const [name, value] of Object.entries(COLORS[theme])) {
    root.style.setProperty(`--${name}`, value)
  }
  for (const [name, colors] of Object.entries(COURSES[theme])) {
    root.style.setProperty(`--course-${name}`, colors.marker)
    root.style.setProperty(`--course-${name}-tint`, colors.tint)
  }
  root.style.colorScheme = theme
  root.style.setProperty('--select-chevron', SELECT_CHEVRON[theme])
}

/** Apply a preference and persist it when browser storage permits. */
export function applyAppearance(theme: AppearanceTheme): void {
  const root = typeof document === 'undefined' ? null : document.documentElement
  if (root === null) return

  root.dataset.theme = theme
  if (theme === 'system') {
    removeExplicitOverrides(root)
    root.style.colorScheme = ''
  } else {
    applyExplicitTheme(root, theme)
  }
  persistTheme(theme)
}

/** Read the local preference and establish the initial appearance before auth data arrives. */
export function initializeAppearance(): void {
  applyAppearance(readStoredTheme() ?? 'system')
}
