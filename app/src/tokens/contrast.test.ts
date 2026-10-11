import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import { COLORS, COURSES, MIN, PAIRS, contrast, pairRatio, tokenColor } from './tokens'

/**
 * The contrast gate the spec asks for by name: every documented text/background
 * token pair must clear 4.5:1, and every icon, border or focus-ring pair 3:1, in
 * **both** themes, or the build fails.
 *
 * Three properties are checked, and the third is the one that keeps the first two
 * honest:
 *
 *   1. every pair in `PAIRS` meets its threshold in both themes;
 *   2. the stylesheet and this module agree on the values, so the documentation
 *      cannot drift from what actually renders;
 *   3. no component contains a raw colour, which is what stops a pair being
 *      bypassed by a literal `#4F46E5` in a `className`.
 */

const appRoot = join(__dirname, '..', '..')
const stylesheet = readFileSync(join(appRoot, 'src', 'styles.css'), 'utf8')

/** Every `--name: #hex` inside a block, as `{ name: '#hex' }`. */
function colourVars(block: string): Record<string, string> {
  const found: Record<string, string> = {}
  for (const match of block.matchAll(/--([a-z][a-z0-9-]*):\s*(#[0-9a-fA-F]{3,8})\s*;/g)) {
    const name = match[1]
    const hex = match[2]
    if (name && hex) found[name] = hex.toLowerCase()
  }
  return found
}

/** The `:root` block of one theme, taken from the stylesheet itself. */
function themeBlock(theme: 'light' | 'dark'): string {
  if (theme === 'light') {
    const start = stylesheet.indexOf(':root {')
    return stylesheet.slice(start, stylesheet.indexOf('}', start))
  }
  const media = stylesheet.indexOf('@media (prefers-color-scheme: dark)')
  expect(media, 'styles.css must declare a dark theme').toBeGreaterThan(-1)
  const start = stylesheet.indexOf(':root {', media)
  return stylesheet.slice(start, stylesheet.indexOf('}', start))
}

describe('token contrast (WCAG 2.2 AA)', () => {
  for (const theme of ['light', 'dark'] as const) {
    describe(theme, () => {
      for (const pair of PAIRS) {
        it(`${pair.fg} on ${pair.bg} clears ${pair.min}:1 — ${pair.where}`, () => {
          const ratio = pairRatio(pair, theme)
          expect(
            ratio,
            `${theme}: ${pair.fg} (${COLORS[theme][pair.fg]}) on ${pair.bg} (${COLORS[theme][pair.bg]}) = ${ratio}:1, needs ${pair.min}:1`,
          ).toBeGreaterThanOrEqual(pair.min)
        })
      }
    })
  }

  it('documents every colour token the stylesheet defines', () => {
    for (const theme of ['light', 'dark'] as const) {
      const declared = colourVars(themeBlock(theme))
      // Course hues live in their own table, so they are compared there.
      const tokenised: Record<string, string> = {}
      for (const [name, hex] of Object.entries(declared)) {
        if (name.startsWith('course-')) continue
        tokenised[name] = hex
      }
      expect(
        Object.keys(tokenised).sort(),
        `${theme}: every colour in styles.css must be in tokens.ts (and vice versa)`,
      ).toEqual(Object.keys(COLORS[theme]).sort())

      for (const [name, hex] of Object.entries(tokenised)) {
        expect(hex, `${theme} --${name} differs between styles.css and tokens.ts`).toBe(
          tokenColor(name, theme),
        )
      }
    }
  })

  it('keeps the six course hues legible and distinct', () => {
    for (const theme of ['light', 'dark'] as const) {
      const names = Object.keys(COURSES[theme])
      expect(names.length, 'six categorical hues, no more').toBe(6)

      for (const [name, color] of Object.entries(COURSES[theme])) {
        // The marker is a 4px bar and the chip's text, so it must clear body text
        // on the ground and on its own tint.
        expect(
          contrast(color.marker, tokenColor('bg', theme)),
          `${theme}: course ${name} marker on the ground`,
        ).toBeGreaterThanOrEqual(MIN.text)
        expect(
          contrast(color.marker, color.tint),
          `${theme}: course ${name} chip text on its tint`,
        ).toBeGreaterThanOrEqual(MIN.text)
        // The tint has to be visibly different from the ground, or the chip is
        // just text with a coloured word.
        expect(
          contrast(color.tint, tokenColor('bg', theme)),
          `${theme}: course ${name} tint vs ground`,
        ).toBeGreaterThan(1.02)
      }

      // Two hues that render the same are one hue with extra steps.
      const markers = Object.values(COURSES[theme]).map((c) => c.marker)
      expect(new Set(markers).size).toBe(6)
    }
  })

  it('has no raw colour in any component', () => {
    // The spec forbids raw hex in components: a literal bypasses the theme and
    // cannot be contrast-checked. Tokens live in `tokens.ts` and `styles.css`.
    const offenders: string[] = []
    const walk = (dir: string) => {
      for (const entry of readdirSync(dir)) {
        const path = join(dir, entry)
        if (statSync(path).isDirectory()) {
          walk(path)
          continue
        }
        if (!/\.(ts|tsx)$/.test(entry)) continue
        if (entry.endsWith('.test.ts') || entry.endsWith('.test.tsx')) continue
        if (path.includes(`${join('src', 'tokens')}`)) continue
        const text = readFileSync(path, 'utf8')
        for (const [, hex] of text.matchAll(/#[0-9a-fA-F]{3,8}\b/g)) {
          offenders.push(`${path.slice(appRoot.length + 1)}: ${hex}`)
        }
      }
    }
    walk(join(appRoot, 'src'))
    expect(offenders, 'components must use tokens, not literal colours').toEqual([])
  })
})
