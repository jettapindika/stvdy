import { readFileSync, readdirSync, statSync } from 'node:fs'
import { join } from 'node:path'

import { describe, expect, it } from 'vitest'

import en from './locales/en.json'
import id from './locales/id.json'
import { EXPANSION, stretch } from './pseudo'

/**
 * The i18n contract.
 *
 * Four failures this catches, all of which are silent in a browser:
 *
 *   1. a key used in a component that does not exist in `en.json` — the screen
 *      renders the key itself, e.g. `chat.composer.send`;
 *   2. a key that exists in English but not in Indonesian — the Indonesian user
 *      silently gets English;
 *   3. a key in the locale files that nothing uses — dead copy that a translator
 *      still has to maintain;
 *   4. a pseudo-locale string that is not actually longer, which would make the
 *      layout check meaningless.
 */

const appRoot = join(__dirname, '..', '..')

/** Every leaf key of a nested resource, dotted. */
function leafKeys(value: unknown, prefix = ''): string[] {
  if (typeof value === 'string') return [prefix]
  if (value && typeof value === 'object') {
    return Object.entries(value as Record<string, unknown>).flatMap(([key, child]) =>
      leafKeys(child, prefix ? `${prefix}.${key}` : key),
    )
  }
  return []
}

/** Every source file under `src`, excluding tests and the locale files themselves. */
function sourceFiles(dir: string): string[] {
  const out: string[] = []
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) {
      out.push(...sourceFiles(path))
      continue
    }
    if (!/\.(ts|tsx)$/.test(entry)) continue
    if (entry.includes('.test.')) continue
    if (path.includes(`${join('src', 'i18n')}`)) continue
    out.push(path)
  }
  return out
}

const english = new Set(leafKeys(en))
const indonesian = new Set(leafKeys(id))

/** The keys `t()` is called with anywhere in the app, as `t('x.y')`. */
function usedKeys(): string[] {
  const keys: string[] = []
  const prefixes: string[] = []
  for (const file of sourceFiles(join(appRoot, 'src'))) {
    const text = readFileSync(file, 'utf8')
    for (const match of text.matchAll(/\bt\(\s*'([a-zA-Z0-9_.]+)'/g)) {
      const key = match[1]
      if (key) keys.push(key)
    }
    // A key handed to a component as a prop (`<NotBuiltRoute whatKey="…" />`) is
    // used just as much as one passed to `t()`, and a screen that resolves it
    // internally would otherwise look like dead copy.
    for (const match of text.matchAll(/\b[a-z][a-zA-Z]*Key="([a-zA-Z0-9_.]+)"/g)) {
      const key = match[1]
      if (key) keys.push(key)
    }
    // A template key (`t(\`styleguide.course.${name}\`)`) is a dynamic lookup, so
    // the static prefix it interpolates into is recorded and every key beneath it
    // counts as used. The alternative — spelling six near-identical keys out in
    // the source — is what the template literal exists to avoid.
    for (const match of text.matchAll(/\bt\(\s*`([a-zA-Z0-9_.]+)\.\$\{/g)) {
      const prefix = match[1]
      if (prefix) prefixes.push(prefix)
    }
  }
  // A dynamic prefix marks everything beneath it as used, which is the honest
  // reading of `t(\`styleguide.course.${name}\`)`.
  for (const prefix of prefixes) {
    for (const key of english) {
      if (key.startsWith(`${prefix}.`)) keys.push(key)
    }
  }
  return keys
}

describe('locale files', () => {
  it('declare exactly the same keys in both languages', () => {
    // Indonesian has no plural form, so it carries `_other` only. Every other key
    // must exist in both, which is what stops an Indonesian user silently
    // getting English.
    const pluralOnly = (key: string) => key.endsWith('_one')
    const missingInId = [...english]
      .filter((key) => !indonesian.has(key) && !pluralOnly(key))
      .sort()
    const missingInEn = [...indonesian].filter((key) => !english.has(key)).sort()
    expect(missingInId, 'keys present in English but not Indonesian').toEqual([])
    expect(missingInEn, 'keys present in Indonesian but not English').toEqual([])
  })

  it('interpolates the same placeholders in both languages', () => {
    // A key can exist in both files and still be broken: an Indonesian string
    // that drops `{{count}}` renders a sentence with the number missing, and one
    // that renames it renders the placeholder literally. i18next resolves a
    // missing variable to nothing, silently, so this is the only check that
    // catches it.
    const placeholders = (value: unknown): string[] => {
      if (typeof value !== 'string') return []
      return [...value.matchAll(/\{\{([a-zA-Z0-9_]+)\}\}/g)].flatMap((m) => (m[1] ? [m[1]] : [])).sort()
    }
    const at = (resource: unknown, key: string) =>
      key.split('.').reduce<unknown>((node, part) => (node as Record<string, unknown>)[part], resource)

    const mismatches: string[] = []
    for (const key of english) {
      if (!indonesian.has(key)) continue
      const inEn = placeholders(at(en, key))
      const inId = placeholders(at(id, key))
      if (inEn.join(',') !== inId.join(',')) {
        mismatches.push(`${key}: en has [${inEn.join(', ')}], id has [${inId.join(', ')}]`)
      }
    }
    expect(mismatches, 'a placeholder that exists in one language but not the other').toEqual([])
  })

  it('are non-empty and keep the required honesty copy', () => {
    // The spec's mandatory composer line, asserted as copy rather than trusted to
    // survive an edit: it is the product's honesty promise, in both languages.
    expect(en.honesty.composerHint).toBe('AI can make mistakes. Check the citations.')
    expect(en.honesty.sourcesOnly).toBe('Answer only from my sources')
    expect(en.honesty.sourcesOnlyTooltip).toContain('check the cited page')
    expect(id.honesty.composerHint.length).toBeGreaterThan(0)
    expect(id.honesty.sourcesOnlyTooltip.length).toBeGreaterThan(0)
  })

  it('has every key the code asks for', () => {
    // A plural call is `t('quiz.count', {count})` while the files hold
    // `quiz.count_one` / `quiz.count_other`, so the stem counts as present.
    const missing = [...new Set(usedKeys())]
      .filter(
        (key) =>
          !english.has(key) && !english.has(`${key}_one`) && !english.has(`${key}_other`),
      )
      .sort()
    expect(missing, 't() keys with no entry in en.json').toEqual([])
  })

  it('has no unused key in a feature namespace', () => {
    const used = new Set(usedKeys())
    // Plural keys are addressed as their stem, so `key_one`/`key_other` count as
    // used when `key` is called with a count.
    const usedWithPlurals = new Set(
      [...used].flatMap((key) => [key, `${key}_one`, `${key}_other`]),
    )

    // The shared vocabulary — `actions`, `nav`, `states`, `honesty`, `lock`,
    // `app`, `shell` — is a design-system contract the next phases render, so a
    // word that no screen uses *yet* is deliberate. A feature namespace is not:
    // copy under `chat.` or `quiz.` that nothing renders is either a leftover or a
    // screen that lost its string, and both are worth failing on.
    const shared = new Set(['app', 'shell', 'nav', 'actions', 'states', 'honesty', 'lock'])

    const unused = [...english]
      .filter((key) => !usedWithPlurals.has(key))
      .filter((key) => !shared.has(key.split('.')[0] ?? ''))
      .sort()
    expect(unused, 'feature copy nothing renders').toEqual([])
  })

  it('makes every pseudo-locale string longer, not just some', () => {
    // The guarantee is about the *text*: a `{{placeholder}}` is copied through
    // untouched (accenting it would stop i18next resolving it), so counting its
    // characters would let a string pass while its visible words stayed short.
    const textOnly = (value: string) => value.replace(/\{\{[^}]*\}\}/g, '')

    for (const key of english) {
      const source = key
        .split('.')
        .reduce<unknown>((node, part) => (node as Record<string, unknown>)[part], en)
      if (typeof source !== 'string') throw new Error(`${key} is not a string`)

      const grown = stretch(source)
      // Placeholders survive verbatim, which is the other half of the contract.
      for (const placeholder of source.match(/\{\{[^}]*\}\}/g) ?? []) {
        expect(grown, `${key} must keep ${placeholder}`).toContain(placeholder)
      }

      const before = textOnly(source).length
      expect(textOnly(grown).length, `${key}: "${grown}"`).toBeGreaterThanOrEqual(
        Math.floor(before * EXPANSION),
      )
      expect(grown, `${key} should not be identical to its English text`).not.toBe(source)
    }
  })
})
