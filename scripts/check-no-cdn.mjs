#!/usr/bin/env node
/**
 * Fails when a production build *loads* something from a third-party origin.
 *
 * The spec makes this a rule with a reason: every runtime asset is bundled, the
 * desktop shell's CSP allows no remote origin, and a phone on campus Wi-Fi must
 * render its own interface. A CDN font, a Google Fonts stylesheet, or an image on
 * a marketing host would each break one of those, and none of them is visible in
 * review because the page still looks right on a developer's machine.
 *
 * Run after `npm --prefix app run build`:
 *
 *   node scripts/check-no-cdn.mjs
 *
 * **It looks for loads, not for URLs.** A bare `https://react.dev/errors/` inside
 * a string is a pointer in an error message that is never fetched, and treating it
 * as a violation would make this check fail on every dependency upgrade until
 * someone added an allowlist entry for each one — which is how a gate becomes
 * noise and then gets deleted. So each shipped extension is checked for the forms
 * that actually cause a request: an element attribute, a CSS `url()`/`@import`, a
 * dynamic `import()`, or a `fetch`/`XMLHttpRequest` call.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const dist = join(root, 'app/dist')

/**
 * Origins a shipped file may load from.
 *
 * `www.w3.org` is an XML namespace, not a fetch: SVG and MathML require it and no
 * browser resolves it. `tauri.localhost` is the shell's own origin, which the app
 * legitimately calls. Anything else here would be a network dependency, so the
 * list is meant to stay this short.
 */
const ALLOWED = ['http://www.w3.org/', 'https://www.w3.org/', 'http://tauri.localhost', 'https://tauri.localhost']

/** The forms that cause a load, per shipped extension. */
const LOADERS = {
  // An element attribute that fetches, and inline `url()` in a style attribute.
  '.html': [/\bsrc\s*=\s*["'](https?:[^"']+)/g, /\bsrcset\s*=\s*["'](https?:[^"']+)/g, /url\(\s*["']?(https?:[^"')]+)/g, /<link\b[^>]*\bhref\s*=\s*["'](https?:[^"']+)/g],
  // A stylesheet can pull a font or an image, or import another sheet.
  '.css': [/url\(\s*["']?(https?:[^"')]+)/g, /@import\s+(?:url\(\s*)?["'](https?:[^"']+)/g],
  // Runtime loading: a dynamic import, a fetch, or an XHR opened at a URL.
  '.js': [/import\s*\(\s*["'](https?:[^"']+)/g, /\bfetch\s*\(\s*["'](https?:[^"']+)/g, /\bopen\s*\(\s*["'][A-Z]+["']\s*,\s*["'](https?:[^"']+)/g, /\.src\s*=\s*["'](https?:[^"']+)/g],
}

/** Extensions that ship and can therefore load something at runtime. */
const SHIPPED = Object.keys(LOADERS)

function* files(dir) {
  for (const entry of readdirSync(dir)) {
    const path = join(dir, entry)
    if (statSync(path).isDirectory()) yield* files(path)
    else yield path
  }
}

let checked = 0
const offenders = []

for (const path of files(dist)) {
  const extension = SHIPPED.find((ext) => path.endsWith(ext))
  if (!extension) continue
  checked += 1
  const text = readFileSync(path, 'utf8')
  for (const pattern of LOADERS[extension]) {
    for (const match of text.matchAll(pattern)) {
      const url = match[1]
      if (!url) continue
      if (ALLOWED.some((allowed) => url.startsWith(allowed))) continue
      offenders.push(`${relative(root, path)}: ${url}`)
    }
  }
}

if (checked === 0) {
  console.error(`no built files found under ${relative(root, dist)} — run the build first`)
  process.exit(1)
}

if (offenders.length > 0) {
  console.error(
    `a production build must not load a third-party origin (${offenders.length} found):`,
  )
  for (const line of offenders) console.error(`  ${line}`)
  console.error('\nBundle the asset, or add the origin to ALLOWED with a reason.')
  process.exit(1)
}

console.log(`no third-party loads in ${checked} shipped file(s)`)
