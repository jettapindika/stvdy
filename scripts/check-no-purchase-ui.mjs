#!/usr/bin/env node
/**
 * Fails when the built app carries a way to buy something.
 *
 * The rule is a store rule, not a preference (roadmap Phase 14 exit gate): the
 * iOS and Android bundles must contain **no purchase UI of any kind**. A review
 * rejects an app that offers an off-store purchase path, and the way that
 * requirement is usually broken is quietly — a price added to a screen, a
 * checkout call added to a client module — because nothing in review reads the
 * bundle. So this check reads the bundle.
 *
 * Run after `npm --prefix app run build`:
 *
 *   node scripts/check-no-purchase-ui.mjs
 *
 * **It looks for affordances, not for words.** The same reasoning
 * `scripts/check-no-cdn.mjs` states for URLs applies here, and it is not
 * theoretical: the shipped bundle already contains the string
 * `lock.upgrade: "Upgrade to {{plan}}"`, the styleguide's description of the
 * two `LockedFeature` variants ("…link to checkout"), and the raw text of
 * `config/plans.toml` and `config/billing_policy.toml` (inlined by the `?raw`
 * imports, comments and `price_idr` keys included). None of those is a purchase
 * path: the locale string is data the mobile policy never renders, and the
 * config text is a table the client reads two fields out of. Flagging them would
 * make this check fail on the current, correct build until someone added an
 * allowlist — which is how a gate becomes noise and then gets deleted.
 *
 * So the denylist holds the forms that *are* a purchase path: a price the user
 * could read, the checkout route or endpoint, and a call to a purchase
 * function. A regression that adds any of them fails here.
 *
 * **Which bytes this inspects is a load-bearing fact, not an implementation
 * detail.** `src-tauri/tauri.conf.json` sets `frontendDist: "../app/dist"`, and
 * there is no per-platform `bundle.iOS`/`bundle.android` override, so
 * `app/dist` is *literally the bundle the iOS and Android shells load* — this is
 * not "the web build standing in for the mobile one". The output names the
 * directory it read for exactly that reason: a route path the phone can never
 * render and a route path the phone *does* register are indistinguishable by
 * regex, so the question "which artifact is this?" has to be answerable from the
 * check's own output rather than from an argument about build targets.
 *
 * The second half of the check is not about bytes at all: `config/billing_policy.toml`
 * must say `false` for all three purchase switches on `ios` and `android`. That
 * file is the switch `LockedFeature` reads, so a bundle with no affordance and a
 * policy that permits one is one edit away from selling; both halves have to hold.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
const dist = join(root, 'app/dist')
const policyFile = join(root, 'config', 'billing_policy.toml')

/**
 * The shipped extensions this reads.
 *
 * `check-no-cdn.mjs` derives its set from the loaders it knows (a file that
 * cannot fetch cannot fetch a CDN); the question here is different — which
 * shipped files can *render* — so this is the text a browser can draw or a
 * script can run. The build emits `.html`, `.js` and `.css` today; `.json` and
 * `.svg` are included because both ship as text and both can carry a price.
 */
const SHIPPED = ['.html', '.js', '.css', '.json', '.svg']

/**
 * What a purchase path looks like in shipped bytes.
 *
 * Each pattern is global because it is used with `matchAll`; `findOffenders`
 * resets `lastIndex` anyway, so the shared regex objects cannot leak state
 * between files.
 */
const MARKERS = [
  { what: 'a rupiah price the user could read', pattern: /\bRp\s?\d[\d.,]*/g },
  { what: 'a rupiah price the user could read', pattern: /\bIDR\s?\d[\d.,]*/g },
  { what: 'the checkout route or endpoint', pattern: /billing\/checkout/g },
  { what: 'a call to a purchase function', pattern: /\b(?:buy|checkout|purchase)\s*\(/g },
]

/** The three switches a mobile policy must leave off. */
const PURCHASE_KEYS = ['allow_purchase_ui', 'show_prices', 'allow_purchase_cta_in_locked_messages']

/** The platforms the stores ship. */
const MOBILE = ['ios', 'android']

/** The marker's surroundings, so a failure names what was found rather than only where. */
function snippet(text, index, length) {
  const from = Math.max(0, index - 30)
  return text.slice(from, Math.min(text.length, index + length + 30)).replace(/\s+/g, ' ')
}

/**
 * Every reason the built app must not ship.
 *
 * Takes the shipped files as a map of path to text rather than reading a
 * directory, so the test can hand it a planted bundle.
 *
 * **An empty map is an offender.** A check that passes because it inspected
 * nothing is the worst outcome available here: it reads as coverage for the one
 * property a store rejection turns on. So a caller that walked a directory that
 * is not there, or a build that emitted nothing this check understands, gets a
 * failure and not a green word.
 */
export function findOffenders(files) {
  const names = Object.keys(files)
  if (names.length === 0) {
    return ['no shipped files were inspected — an empty build proves nothing about purchase UI']
  }

  const offenders = []
  for (const name of names.sort()) {
    const text = files[name]
    for (const marker of MARKERS) {
      marker.pattern.lastIndex = 0
      for (const match of text.matchAll(marker.pattern)) {
        offenders.push(`${name}: ${marker.what} — "${snippet(text, match.index, match[0].length)}"`)
      }
    }
  }
  return offenders
}

/** The lines of one `[platforms.<name>]` block, comments stripped, or `null`. */
function platformBlock(toml, platform) {
  const lines = []
  let inside = false
  for (const rawLine of toml.split('\n')) {
    const line = (rawLine.split('#')[0] ?? '').trim()
    if (line === '') continue
    if (line.startsWith('[')) {
      inside = line === `[platforms.${platform}]`
      continue
    }
    if (inside) lines.push(line)
  }
  return lines.length > 0 ? lines : null
}

/**
 * Every reason the policy file permits a purchase path on a store platform.
 *
 * This is a second reader of `config/billing_policy.toml`, and the duplication
 * is forced rather than chosen: the app's reader (`features/billing/policy.ts`)
 * is TypeScript behind Vite's `@config` alias, so a plain Node script cannot
 * import it. It parses the same one shape that file has, and it is deliberately
 * stricter in one direction — a missing block or a missing key is an offender,
 * not a default, because a store rule that rests on a default is one deleted
 * line away from being off.
 */
export function policyOffenders(toml) {
  const offenders = []
  for (const platform of MOBILE) {
    const block = platformBlock(toml, platform)
    if (block === null) {
      offenders.push(`[platforms.${platform}] is missing from billing_policy.toml`)
      continue
    }
    for (const key of PURCHASE_KEYS) {
      const line = block.find((candidate) => new RegExp(`^${key}\\s*=`).test(candidate))
      if (line === undefined) {
        offenders.push(`[platforms.${platform}] does not declare ${key}`)
        continue
      }
      const value = /=\s*(true|false)\s*$/.exec(line)?.[1]
      if (value === undefined) {
        offenders.push(`[platforms.${platform}] ${key} is not a boolean — "${line}"`)
        continue
      }
      if (value === 'true') offenders.push(`[platforms.${platform}] ${key} = true`)
    }
  }
  return offenders
}

/** Every shipped file under `dir`, as a map of repo-relative path to text. */
function readShipped(dir) {
  const files = {}
  const walk = (current) => {
    for (const entry of readdirSync(current)) {
      const path = join(current, entry)
      if (statSync(path).isDirectory()) {
        walk(path)
        continue
      }
      if (!SHIPPED.some((extension) => path.endsWith(extension))) continue
      files[relative(root, path)] = readFileSync(path, 'utf8')
    }
  }
  walk(dir)
  return files
}

/** True when this module was run as a script rather than imported by the test. */
function isCli() {
  return process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href
}

if (isCli()) {
  let files
  try {
    files = readShipped(dist)
  } catch (error) {
    console.error(`${relative(root, dist)} could not be read (${error.message}) — run the build first`)
    process.exit(1)
  }

  const policy = readFileSync(policyFile, 'utf8')
  const offenders = [...findOffenders(files), ...policyOffenders(policy)]

  if (offenders.length > 0) {
    console.error(`a mobile build must contain no purchase UI (${offenders.length} found):`)
    for (const line of offenders) console.error(`  ${line}`)
    console.error(
      Object.keys(files).length === 0
        ? '\nNothing shipped was inspected: run the build first.'
        : '\nRemove the affordance, or fix config/billing_policy.toml for ios and android.',
    )
    process.exit(1)
  }

  console.log(`no purchase UI in ${Object.keys(files).length} shipped file(s) under ${relative(root, dist)}`)
  console.log(`billing_policy.toml denies purchase UI, prices and locked-message CTAs on ${MOBILE.join(' and ')}`)
}
