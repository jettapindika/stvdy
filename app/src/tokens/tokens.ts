/**
 * The design tokens, as data.
 *
 * `styles.css` supplies the system palette; explicit appearance preferences
 * apply these same values as root overrides. This module also describes the
 * colour pairs and names used by components:
 *
 *   1. **Prove the contrast.** `contrast.test.ts` computes every documented
 *      text/background pair in both themes and fails the build on a miss. The
 *      spec asks for that check by name, and it only means something if the pairs
 *      are declared rather than eyeballed.
 *   2. **Let a component ask for a colour.** The course markers pick one of six
 *      hues by index, and the citation popover names the source pair. Both need
 *      the token *names* rather than a literal, so the value stays theme-aware.
 *
 * Keep this file in step with `styles.css`: `contrast.test.ts` also parses the
 * stylesheet and fails if a variable exists in one and not the other, which is
 * the drift that would otherwise be invisible until a component rendered wrong.
 */

/** WCAG 2.2 AA minimums. `Ui` covers icons, borders and other non-text. */
export const MIN = {
  /** Body and small text. */
  text: 4.5,
  /** Large text (>= 18.66px bold or >= 24px) and UI components/graphics. */
  large: 3,
  ui: 3,
} as const

export type Theme = 'light' | 'dark'

/**
 * Every colour token, per theme.
 *
 * The values are duplicated from `styles.css` on purpose: this file is the
 * *assertion* side, and a test that read the CSS would pass even if the CSS and
 * the documentation disagreed. `contrast.test.ts` closes the loop by parsing the
 * stylesheet and comparing.
 */
export const COLORS: Record<Theme, Record<string, string>> = {
  light: {
    bg: '#f5f5f7',
    surface: '#ffffff',
    'surface-raised': '#ebebef',
    border: '#dcdce3',
    'border-strong': '#7e7e8e',
    text: '#151518',
    'text-muted': '#4f4f5b',
    'text-subtle': '#686874',
    primary: '#4f46e5',
    'primary-hover': '#3525cd',
    'on-primary': '#ffffff',
    'primary-text': '#3525cd',
    'primary-tint': '#e2dfff',
    source: '#006591',
    'source-tint': '#c9e6ff',
    success: '#005338',
    'success-tint': '#d5f5e8',
    warning: '#92400e',
    'warning-tint': '#fef3c7',
    danger: '#ba1a1a',
    'danger-tint': '#ffdad6',
    ring: '#4f46e5',
  },
  dark: {
    bg: '#080809',
    surface: '#161618',
    'surface-raised': '#232326',
    border: '#323237',
    'border-strong': '#838390',
    text: '#f7f7fa',
    'text-muted': '#c2c2cc',
    'text-subtle': '#a0a0ad',
    primary: '#4f46e5',
    'primary-hover': '#3f37c9',
    'on-primary': '#ffffff',
    'primary-text': '#c4bfff',
    'primary-tint': '#28234d',
    source: '#8cd3ff',
    'source-tint': '#0d2b40',
    success: '#85e0ba',
    'success-tint': '#10382d',
    warning: '#f8d787',
    'warning-tint': '#392c16',
    danger: '#ffb4ab',
    'danger-tint': '#432327',
    ring: '#c4bfff',
  },
}

/**
 * Every pair the UI actually renders, with the threshold it must clear.
 *
 * This is the honest form of the spec's requirement: not "the palette is
 * accessible" but "these are the combinations on screen, and here is each one's
 * measured ratio". A pair that is not used should not be listed; a pair that is
 * used must be.
 */
export interface Pair {
  /** `fg` token name. */
  fg: string
  /** `bg` token name. */
  bg: string
  min: number
  /** Where this combination appears, so a failure is actionable. */
  where: string
}

export const PAIRS: Pair[] = [
  // Body and metadata text on each ground.
  { fg: 'text', bg: 'bg', min: MIN.text, where: 'body text on the page ground' },
  { fg: 'text', bg: 'surface', min: MIN.text, where: 'body text on a card' },
  { fg: 'text', bg: 'surface-raised', min: MIN.text, where: 'body text on a raised block' },
  { fg: 'text-muted', bg: 'bg', min: MIN.text, where: 'secondary text on the ground' },
  { fg: 'text-muted', bg: 'surface', min: MIN.text, where: 'secondary text on a card' },
  { fg: 'text-muted', bg: 'surface-raised', min: MIN.text, where: 'secondary text on a raised block' },
  { fg: 'text-subtle', bg: 'bg', min: MIN.text, where: 'composer hint on the ground' },
  { fg: 'text-subtle', bg: 'surface', min: MIN.text, where: 'caption on a card' },
  { fg: 'text-subtle', bg: 'surface-raised', min: MIN.text, where: 'caption on a raised block' },

  // Actions and brand.
  { fg: 'on-primary', bg: 'primary', min: MIN.text, where: 'the label on a primary button' },
  // The hover fill, declared because a pair the gate does not measure is a pair that
  // drifts: using `primary-text` as the hover fill put white on `#C4BFFF` in dark
  // (1.72:1) while every declared pair stayed green.
  {
    fg: 'on-primary',
    bg: 'primary-hover',
    min: MIN.text,
    where: 'the label on a primary button, hovered',
  },
  { fg: 'primary-text', bg: 'bg', min: MIN.text, where: 'a link in body copy' },
  { fg: 'primary-text', bg: 'surface', min: MIN.text, where: 'a link on a card' },
  { fg: 'primary-text', bg: 'surface-raised', min: MIN.text, where: 'secondary actions and Settings icon tiles' },
  { fg: 'primary-text', bg: 'primary-tint', min: MIN.text, where: 'the active nav item' },

  // Sources and citations: the pair that must mean "this came from your material".
  { fg: 'source', bg: 'source-tint', min: MIN.text, where: 'a citation chip' },
  { fg: 'source', bg: 'surface', min: MIN.text, where: 'a source label on a card' },
  { fg: 'bg', bg: 'source', min: MIN.text, where: 'a citation chip on hover (inverted)' },

  // State.
  { fg: 'success', bg: 'success-tint', min: MIN.text, where: 'a "ready" chip' },
  { fg: 'warning', bg: 'warning-tint', min: MIN.text, where: 'a "due" chip' },
  { fg: 'warning', bg: 'surface', min: MIN.text, where: 'a "due today" label on a card' },
  // A deadline's timeline row is drawn on the warning tint (`today`), and the two text
  // levels it carries are the body levels rather than the warning colour: the row's
  // title is `.text-label` and its course line is `.text-caption text-muted`. Declared
  // because this file's rule is that a rendered pair is named here rather than eyeballed,
  // and because the row is the only surface that puts body text on a warning ground.
  { fg: 'text', bg: 'warning-tint', min: MIN.text, where: "a deadline row's title" },
  { fg: 'text-muted', bg: 'warning-tint', min: MIN.text, where: "a deadline row's course line" },
  { fg: 'danger', bg: 'danger-tint', min: MIN.text, where: 'an error note' },
  { fg: 'danger', bg: 'surface', min: MIN.text, where: 'an error message on a card' },
  { fg: 'bg', bg: 'danger', min: MIN.text, where: 'the label on a destructive button' },

  // Non-text: a boundary has to be a real edge, and a focus ring has to be visible.
  { fg: 'border-strong', bg: 'surface', min: MIN.ui, where: 'an input boundary' },
  { fg: 'border-strong', bg: 'surface', min: MIN.ui, where: 'a secondary button boundary' },
  { fg: 'border-strong', bg: 'bg', min: MIN.ui, where: 'an input boundary on the ground' },
  { fg: 'ring', bg: 'bg', min: MIN.ui, where: 'the focus ring on the ground' },
  { fg: 'ring', bg: 'surface', min: MIN.ui, where: 'the focus ring on a card' },
  { fg: 'ring', bg: 'surface-raised', min: MIN.ui, where: 'the focus ring on a raised block' },

  // The QRIS code, and it is listed at the **text** threshold rather than the 3:1
  // graphics one. `CheckoutScreen`'s `QrCode` draws the modules with `text-primary`
  // (so `currentColor` is `--primary`) on a `bg-on-primary` ground — the one pair in
  // the app that must NOT follow the theme, because light modules on a dark ground do
  // not scan. The ratio is the same number as the primary button's label, since
  // contrast is symmetric, and it is declared separately anyway: it is a different
  // surface with a different failure mode. A label that is hard to read is a
  // complaint; a code that will not scan is a student who cannot pay, and 4.5:1 is
  // the threshold a code read by a camera has to clear, not the 3:1 one that covers
  // an icon.
  { fg: 'primary', bg: 'on-primary', min: MIN.text, where: 'the QRIS modules on their quiet zone' },
]

/** The six course hues and their tints, per theme. */
export const COURSES: Record<Theme, Record<string, { marker: string; tint: string }>> = {
  light: {
    indigo: { marker: '#4f46e5', tint: '#e2dfff' },
    sky: { marker: '#0369a1', tint: '#c9e6ff' },
    emerald: { marker: '#047857', tint: '#d5f5e8' },
    amber: { marker: '#92400e', tint: '#fbeab5' },
    rose: { marker: '#be123c', tint: '#ffdad6' },
    violet: { marker: '#6d28d9', tint: '#ede4ff' },
  },
  dark: {
    indigo: { marker: '#c4bfff', tint: '#28234d' },
    sky: { marker: '#8cd3ff', tint: '#0d2b40' },
    emerald: { marker: '#85e0ba', tint: '#10382d' },
    amber: { marker: '#f8d787', tint: '#392c16' },
    rose: { marker: '#ffb4ab', tint: '#432327' },
    violet: { marker: '#c4b5fd', tint: '#2a1b4d' },
  },
}

/** The course hues, in the order the picker offers them. */
export const COURSE_NAMES = ['indigo', 'sky', 'emerald', 'amber', 'rose', 'violet'] as const
export type CourseName = (typeof COURSE_NAMES)[number]

/** True when `value` is one of the six course hues. */
function isCourseName(value: string): value is CourseName {
  return (COURSE_NAMES as readonly string[]).includes(value)
}

/**
 * The CSS custom properties for a course colour, ready to spread into `style`.
 *
 * **It emits variable *references*, not literals, and that is the whole point.** The
 * app has no runtime theme — the theme is a `prefers-color-scheme` block in the
 * stylesheet — so a caller that inlined `COURSES.light.indigo` painted the *light* pair
 * on a dark page. That was not theoretical: every caller passed one argument, so the old
 * `theme = 'light'` default meant the light indigo marker (`#4f46e5`) rendered on the dark
 * ground at **3.05:1**, under the 4.5:1 `contrast.test.ts` asserts for the dark theme's
 * own values, and the gate could not see it because it reads `COURSES` directly rather
 * than through this function. References resolve per theme, so there is now nothing to get
 * wrong at a call site and no theme parameter to default incorrectly.
 *
 * `COURSES` remains the *assertion* side, which is what the contrast gate compares;
 * `COURSE_VARS` below is the reference table this reads.
 */
export function courseVars(name: string): Record<string, string> {
  const { marker, tint } = COURSE_VARS[isCourseName(name) ? name : 'indigo']
  return { '--course-marker': marker, '--course-tint': tint }
}

/**
 * The six course hues as CSS variable *references*, for a component that paints
 * its own chip rather than setting the pair on an ancestor.
 *
 * `COURSES` above holds the literals, which is what the contrast gate compares;
 * these are references, and the same hue resolves to a different value inside the
 * dark block. The distinction matters because the app has no runtime theme — the
 * theme is a `prefers-color-scheme` block in the stylesheet — so a component that
 * inlined `COURSES.light.indigo` would paint the light pair on a dark page, and
 * every screenshot in the design is light, which is why that mistake survives
 * review.
 *
 * The hue names are the ones `styles.css` declares; `invariants.test.ts` fails the
 * build if one of these reads a property nothing declares.
 */
export const COURSE_VARS: Record<CourseName, { marker: string; tint: string }> = {
  indigo: { marker: 'var(--course-indigo)', tint: 'var(--course-indigo-tint)' },
  sky: { marker: 'var(--course-sky)', tint: 'var(--course-sky-tint)' },
  emerald: { marker: 'var(--course-emerald)', tint: 'var(--course-emerald-tint)' },
  amber: { marker: 'var(--course-amber)', tint: 'var(--course-amber-tint)' },
  rose: { marker: 'var(--course-rose)', tint: 'var(--course-rose-tint)' },
  violet: { marker: 'var(--course-violet)', tint: 'var(--course-violet-tint)' },
}

/**
 * A colour channel's linear value, per WCAG 2.2.
 *
 * A malformed hex would otherwise produce `NaN` and a ratio that compares
 * `false` against every threshold — which reads as a failure, so the safe
 * direction is preserved even if the input is wrong.
 */
function linear(hex: string, offset: number): number {
  const channel = Number.parseInt(hex.slice(offset, offset + 2), 16) / 255
  if (Number.isNaN(channel)) return 0
  return channel <= 0.04045 ? channel / 12.92 : ((channel + 0.055) / 1.055) ** 2.4
}

/** Relative luminance, per WCAG 2.2. */
function luminance(hex: string): number {
  const value = hex.replace('#', '')
  return 0.2126 * linear(value, 0) + 0.7152 * linear(value, 2) + 0.0722 * linear(value, 4)
}

/** The contrast ratio between two hex colours, rounded to two decimals. */
export function contrast(a: string, b: string): number {
  const first = luminance(a)
  const second = luminance(b)
  const hi = Math.max(first, second)
  const lo = Math.min(first, second)
  return Math.round(((hi + 0.05) / (lo + 0.05)) * 100) / 100
}

/**
 * The ratio for a declared pair in a theme.
 *
 * A token name that is not in the table throws rather than returning a number:
 * a typo in `PAIRS` would otherwise be measured as black-on-black and reported as
 * a failure of the wrong thing.
 */
export function pairRatio(pair: Pair, theme: Theme): number {
  const colors = COLORS[theme]
  const fg = colors[pair.fg]
  const bg = colors[pair.bg]
  if (!fg || !bg) {
    throw new Error(`unknown token in PAIRS: ${pair.fg} on ${pair.bg}`)
  }
  return contrast(fg, bg)
}

/** The declared value of a token, or a throw naming it. */
export function tokenColor(name: string, theme: Theme): string {
  const hex = COLORS[theme][name]
  if (!hex) throw new Error(`unknown colour token: ${name}`)
  return hex
}
