import { useSyncExternalStore } from 'react'

/**
 * Layout form factor, derived from viewport width.
 *
 * This is deliberately NOT part of the platform adapter. It used to be: each
 * adapter hardcoded its own answer, which meant four copies of one mapping and
 * one of them was wrong. Worse, the mapping was keyed on platform *kind*, so a
 * phone browser reported `web` and the app rendered neither the sidebar (hidden
 * below 640px by CSS) nor the tab bar. No navigation at all on a phone.
 *
 * Width is also the more correct signal: an iPad and a narrow desktop window are
 * the same layout problem, and a native app on a tablet wants the tablet layout
 * regardless of which adapter it uses.
 *
 * The breakpoints here must match the CSS ones: 768px (`md`) and 1024px (`lg`).
 * If they drift, the JS and CSS disagree about which shell to render and the
 * result is exactly the bug above.
 */

export type FormFactor = 'phone' | 'tablet' | 'desktop'

const TABLET_MIN = 768
const DESKTOP_MIN = 1024

export function formFactorForWidth(width: number): FormFactor {
  if (width >= DESKTOP_MIN) return 'desktop'
  if (width >= TABLET_MIN) return 'tablet'
  return 'phone'
}

function subscribe(onChange: () => void): () => void {
  // `resize` alone is enough and fires for the cases that matter (window resize,
  // orientation change). Listening on the media queries too makes the transition
  // exact at the boundary rather than dependent on a resize event landing.
  const queries = [
    window.matchMedia(`(min-width: ${TABLET_MIN}px)`),
    window.matchMedia(`(min-width: ${DESKTOP_MIN}px)`),
  ]
  for (const query of queries) query.addEventListener('change', onChange)
  window.addEventListener('resize', onChange)
  return () => {
    for (const query of queries) query.removeEventListener('change', onChange)
    window.removeEventListener('resize', onChange)
  }
}

function getSnapshot(): FormFactor {
  return formFactorForWidth(window.innerWidth)
}

export function useFormFactor(): FormFactor {
  // `useSyncExternalStore` rather than state + effect: the value is read from an
  // external source, and this keeps it correct on the first render (no flash of
  // the wrong shell) as well as after a resize.
  return useSyncExternalStore(subscribe, getSnapshot, () => 'desktop')
}
