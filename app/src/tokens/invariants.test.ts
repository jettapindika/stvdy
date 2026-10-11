import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

/**
 * The design-system invariants the contrast test cannot see.
 *
 * A contrast test measures the *values* of the token pairs, so it passes while a
 * component reaches for the wrong utility, and the raw-hex scan sees no literal
 * either. Two defects shipped through exactly that hole and both are asserted
 * here, along with the general forms of them:
 *
 *   1. **Focus uses `--ring`.** `--primary` is the same `#4f46e5` in both themes,
 *      which is 2.82:1 on the dark surface — the measured failure `--ring` exists
 *      for (A102). A component that reaches for `outline-primary` reintroduces it.
 *   2. **Every `var(--x)` is defined.** A rename this wide is when an undefined
 *      token ships, and CSS silently inherits instead of failing.
 *   3. **`.course-marker` reads the per-course variable**, or six courses render
 *      one colour.
 */

const appRoot = join(__dirname, '..', '..')
const stylesheet = readFileSync(join(appRoot, 'src', 'styles.css'), 'utf8')

/** Every source file under `src`, excluding tests. */
function sourceFiles(dir: string): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) {
      out.push(...sourceFiles(path))
      continue
    }
    if (!/\.(ts|tsx|css)$/.test(entry)) continue
    if (entry.includes('.test.')) continue
    out.push(path)
  }
  return out
}

const files = sourceFiles(join(appRoot, 'src'))

/** The `--name` a `var(--name…)` reads, without the fallback. */
function readsIn(text: string): string[] {
  return [...text.matchAll(/var\(\s*--([a-z][a-z0-9-]*)/g)].flatMap((m) => (m[1] ? [m[1]] : []))
}

/**
 * Every custom property the app declares.
 *
 * Two places count: `styles.css` (the theme blocks) and a component's inline
 * `style` object, which is how the course colours are set per row
 * (`courseVars()` writes `--course-marker` / `--course-tint` on the element).
 * A property declared only inline is still declared, so a `var()` reading it is
 * not a defect; what the check is for is a name that nothing declares anywhere.
 */
function declared(): Set<string> {
  const names = new Set<string>()
  for (const match of stylesheet.matchAll(/--([a-z][a-z0-9-]*)\s*:/g)) {
    if (match[1]) names.add(match[1])
  }
  for (const path of files) {
    // `.ts` as well as `.tsx`: `courseVars()` in `tokens.ts` is what writes the
    // per-course properties, and it is a plain module.
    if (!path.endsWith('.tsx') && !path.endsWith('.ts')) continue
    const text = readFileSync(path, 'utf8')
    for (const match of text.matchAll(/'(--[a-z][a-z0-9-]*)':/g)) {
      if (match[1]) names.add(match[1].slice(2))
    }
  }
  return names
}

describe('design-system invariants', () => {
  it('focuses with the ring token, never with the fill colour', () => {
    // `outline-primary` is the failure this rule exists for: `--primary` on the
    // dark surface is 2.82:1, below the 3:1 a focus indicator needs.
    const offenders: string[] = []
    for (const path of files) {
      if (!path.endsWith('.tsx')) continue
      const text = readFileSync(path, 'utf8')
      // `outline-offset-2` and friends match the same shape but are geometry, not
      // colour, so the rule is scoped to the colour utilities.
      for (const match of text.matchAll(/\b(?:focus|has-\[:focus-visible\]):outline-([a-z-]+)/g)) {
        const utility = match[1] ?? ''
        if (['offset', 'width', 'style', 'none', 'dashed', 'solid'].some((word) => utility.startsWith(word))) {
          continue
        }
        if (utility !== 'ring') {
          offenders.push(`${path.slice(appRoot.length + 1)}: outline-${utility}`)
        }
      }
    }
    expect(offenders, 'a focus ring must use outline-ring').toEqual([])
  })

  it('uses the ring token at least once, so the rule above is not vacuous', () => {
    const ring = files.filter(
      (path) => path.endsWith('.tsx') && readFileSync(path, 'utf8').includes('outline-ring'),
    )
    expect(ring.length, 'no component uses the focus ring token').toBeGreaterThan(0)
  })

  it('defines every custom property it reads', () => {
    const known = declared()
    // Tailwind's own theme variables (`--color-*`, `--font-*`, `--text-*`,
    // `--radius-*`, `--spacing-*`, `--duration-*`, `--ease-*`) are declared in the
    // `@theme` block, which the regex above already covers, and `--tw-*` belongs
    // to Tailwind's generated utilities rather than to this file.
    const offenders: string[] = []
    for (const path of files) {
      for (const name of readsIn(readFileSync(path, 'utf8'))) {
        if (known.has(name)) continue
        if (name.startsWith('tw-')) continue
        offenders.push(`${path.slice(appRoot.length + 1)}: var(--${name})`)
      }
    }
    expect([...new Set(offenders)].sort(), 'a token is read but never declared').toEqual([])
  })

  it('falls back to the shared course hue when no per-course variable is set', () => {
    // The defect: `.course-marker` read `--course-indigo` unconditionally, so all
    // six gallery swatches rendered the same bar while `courseVars()` set
    // `--course-marker` inline on an ancestor that nothing consulted.
    const rule = /\.course-marker\s*\{[^}]*\}/.exec(stylesheet)?.[0] ?? ''
    expect(rule, '.course-marker must exist').not.toBe('')
    expect(rule, '.course-marker must read --course-marker, with --course-indigo only as the fallback').toMatch(
      /var\(\s*--course-marker\s*,\s*var\(\s*--course-indigo\s*\)\s*\)/,
    )
  })

  it('sets the per-course variables the marker reads', () => {
    // The other half: the fallback only works if something writes the variable.
    const tokens = readFileSync(join(appRoot, 'src', 'tokens', 'tokens.ts'), 'utf8')
    expect(tokens).toContain("'--course-marker': marker")
    expect(tokens).toContain("'--course-tint': tint")
  })

  it('bundles a font face for every family the theme names', () => {
    // `--font-mono` silently falls back to system mono if JetBrains Mono is not
    // bundled, and a fallback is exactly what the spec's "bundled locally, no
    // network" rule forbids. The script that generates `fonts.css` shipped once
    // without JetBrains Mono and its summary line still claimed success.
    const fonts = readFileSync(join(appRoot, 'src', 'fonts.css'), 'utf8')
    const families = [...stylesheet.matchAll(/--font-([a-z]+):\s*"([^"]+)"/g)].map((m) => m[2] ?? '')
    expect(families.length, 'the theme must name its font families').toBeGreaterThanOrEqual(3)

    for (const family of families) {
      expect(fonts, `${family} must have a bundled @font-face`).toContain(`font-family: '${family}'`)
    }
    // And each face must point at a file that is actually in the bundle.
    for (const match of fonts.matchAll(/src: url\('\/fonts\/([^']+)'\)/g)) {
      const file = match[1]
      expect(file, 'a @font-face with no file').toBeTruthy()
      const path = join(appRoot, 'public', 'fonts', String(file))
      expect(statSync(path).isFile(), `${file} is missing from app/public/fonts`).toBe(true)
    }
  })
})
