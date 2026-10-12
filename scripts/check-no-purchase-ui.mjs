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
 * Run after the *store* build. On a phone `tauri build` runs it for you — its
 * `beforeBuildCommand` is `npm run build`, and the CLI sets `TAURI_ENV_PLATFORM` to
 * the target it is packaging, which is what `vite.config.ts` keys the store build
 * on. Without a shell, the same build is one command:
 *
 *   npm --prefix app run build:mobile                 # writes dist-mobile
 *   node scripts/check-no-purchase-ui.mjs app/dist-mobile
 *
 *   TAURI_ENV_PLATFORM=ios npm --prefix app run build  # what `tauri build` runs
 *   node scripts/check-no-purchase-ui.mjs              # reads app/dist
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
 * detail, and the artifact answers it about itself.** Every build stamps its
 * `index.html` with `stvdy-build-target` (`vite.config.ts`): `store` for the bundle
 * iOS and Android receive, `full` for the web/desktop one. This check reads that
 * stamp and **refuses to pass on anything but a `store` artifact**, because the
 * property it asserts is platform-conditional — `config/billing_policy.toml`
 * permits a checkout route on web and desktop and forbids it on the stores — so a
 * green result from a `full` build would be a control that looks enforced while the
 * property stayed false. That refusal is not a formality: it is the defect this
 * check was written through, when `app/dist` was the web build and the check called
 * it the mobile one.
 *
 * The stamp is the artifact's own claim rather than the caller's. A directory name
 * is a convention, and an env var is what the caller says about a build it may not
 * have made; Vite rewrites `index.html` on every build, so a `store` stamp cannot
 * be inherited from an earlier one. It says `store` only when the shell is
 * packaging for a store platform (`TAURI_ENV_PLATFORM` is `ios` or `android`, which
 * the Tauri CLI sets for `beforeBuildCommand` — so the bundle `frontendDist`
 * packages is the stamped one) or when `VITE_PLATFORM=mobile` asked for the same
 * build without a shell.
 *
 * The output names both the directory and the variant it read, so a green result
 * cannot be attributed to the wrong artifact.
 *
 * The second half of the check is not about bytes at all: `config/billing_policy.toml`
 * must say `false` for all three purchase switches on `ios` and `android`. That
 * file is the switch `LockedFeature` reads, so a bundle with no affordance and a
 * policy that permits one is one edit away from selling; both halves have to hold.
 * That half is platform-independent and runs whichever artifact is read.
 */
import { readFileSync, readdirSync, statSync } from 'node:fs'
import { dirname, join, relative, resolve } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')
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
 * How a build identifies itself, and which identity this check will accept.
 *
 * `vite.config.ts` injects `<meta name="stvdy-build-target" content="…">` into
 * every `index.html` it emits, with one of two values: `store` for the bundle the
 * iOS and Android shells package, `full` for the web/desktop build. This check
 * reads that stamp out of the artifact and refuses to pass on anything else,
 * because the property it asserts is platform-conditional (a checkout route is
 * legitimate on web and desktop and forbidden on the stores) — so a pass on a
 * `full` build would be a control that looks enforced while the property stayed
 * false, which is the exact failure this check was written through.
 */
const STAMP_ATTR = 'stvdy-build-target'
const STORE_VARIANT = 'store'
const FULL_VARIANT = 'full'

/** One `<meta name="stvdy-build-target" …>` tag, wherever its attributes sit. */
const STAMP_TAG = new RegExp(`<meta[^>]*\\bname=["']${STAMP_ATTR}["'][^>]*>`, 'i')

/** The `content` of a stamp tag, or `null` when the tag has none. */
function stampContent(tag) {
  return /content=["']([^"']*)["']/i.exec(tag)?.[1] ?? null
}

/**
 * The variant the artifact says it is, or `null` when it does not say.
 *
 * Every `.html` in the bundle is tried, because the file the stamp lands in is
 * Vite's business (`index.html` today) and hard-coding that name would make this
 * gate fail on a build config change rather than on the property. A build with no
 * stamp at all is not a build this check can identify, which is a failure and not
 * a default (see `artifactOffenders`).
 */
export function buildVariant(files) {
  for (const name of Object.keys(files).sort()) {
    if (!name.endsWith('.html')) continue
    const tag = STAMP_TAG.exec(files[name])?.[0]
    if (tag === undefined) continue
    return stampContent(tag)
  }
  return null
}

/**
 * Every reason the artifact is not the one a store receives.
 *
 * This is the half that makes the byte scan mean something. `findOffenders` can
 * only say "these bytes carry no purchase path"; it cannot say whether those bytes
 * are the ones a phone installs, and the two questions have different answers —
 * `app/dist` holds a checkout route whenever the shell packaged for web or
 * desktop, and holds none when it packaged for iOS or Android.
 *
 * **A missing stamp is an offender, not a pass.** That is the `dist-mobile`-that-
 * nobody-ships case: an artifact this check cannot identify is an artifact whose
 * cleanliness proves nothing, and a gate that greens on it is the original defect
 * with a new filename. The one caller that may pass an unidentified artifact is
 * `scripts/check-mobile-no-purchase.sh`'s own probe, and that probe *wants* the
 * failure.
 *
 * An empty map returns no offenders here: "nothing was inspected" is
 * `findOffenders`' message, and a second copy of it would be a second answer to
 * one question.
 */
export function artifactOffenders(files) {
  if (Object.keys(files).length === 0) return []

  const variant = buildVariant(files)
  if (variant === STORE_VARIANT) return []
  if (variant === null) {
    return [
      `no ${STAMP_ATTR} stamp in the artifact — this is not a build this check can identify, and a pass on an unidentified artifact proves nothing about what a store receives`,
    ]
  }
  if (variant === FULL_VARIANT) {
    return [
      `the artifact is stamped ${STAMP_ATTR}="${FULL_VARIANT}" — this is the web/desktop build, where a checkout route is legitimate, so a pass here would say nothing about the bundle a phone installs. Build the store artifact (TAURI_ENV_PLATFORM=ios npm run build, or npm run build:mobile) and inspect that`,
    ]
  }
  return [
    `the artifact is stamped ${STAMP_ATTR}="${variant}", which is not a variant this check knows — expected "${STORE_VARIANT}"`,
  ]
}

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

/**
 * Which build to inspect.
 *
 * Defaults to `app/dist`, which is the directory `src-tauri/tauri.conf.json`'s
 * `frontendDist` packages — and therefore holds the store bundle exactly when the
 * shell packaged for iOS or Android (`tauri build` runs this build itself, with
 * `TAURI_ENV_PLATFORM` set to the target). An explicit directory overrides it:
 *
 *   node scripts/check-no-purchase-ui.mjs                  # app/dist
 *   node scripts/check-no-purchase-ui.mjs app/dist-mobile  # a shell-less store build
 *
 * **A directory argument is not a claim about what is in it.** The check does not
 * take the caller's word for which build it is reading — it reads the
 * `stvdy-build-target` stamp out of the artifact and fails on anything but
 * `store` (see `artifactOffenders`). So the argument only says *where* to look,
 * and the artifact says *what* it is.
 *
 * A relative argument resolves against the repository root rather than the
 * process's working directory, because that is what the forms above mean and it
 * is what a `package.json` script needs: `npm run` sets the working directory to
 * the package, so a cwd-relative `app/dist-mobile` would resolve to
 * `app/app/dist-mobile` from there. An absolute argument is taken as given —
 * `resolve` returns it unchanged, which is what makes
 * `node scripts/check-no-purchase-ui.mjs /tmp/some-artifact` work rather than
 * silently resolving it to `<repo>/tmp/some-artifact` and inspecting nothing.
 * That silent misjoin is the failure this whole check exists to prevent, so the
 * resolution has to be right rather than conventional.
 */
const dist =
  process.argv[2] === undefined ? join(root, 'app/dist') : resolve(root, process.argv[2])

if (isCli()) {
  let files
  try {
    files = readShipped(dist)
  } catch (error) {
    console.error(`${relative(root, dist)} could not be read (${error.message}) — run the build first`)
    process.exit(1)
  }

  const policy = readFileSync(policyFile, 'utf8')
  const offenders = [
    ...artifactOffenders(files),
    ...findOffenders(files),
    ...policyOffenders(policy),
  ]

  if (offenders.length > 0) {
    console.error(`a store build must contain no purchase UI (${offenders.length} found):`)
    for (const line of offenders) console.error(`  ${line}`)
    console.error(
      Object.keys(files).length === 0
        ? '\nNothing shipped was inspected: run the build first.'
        : '\nRemove the affordance, or fix config/billing_policy.toml for ios and android.',
    )
    process.exit(1)
  }

  // The count stays on its own line with nothing after it: this exact line is the
  // contract `scripts/check-mobile-no-purchase.sh` reads to report what was
  // inspected, and an evidence script whose "what did you inspect" claim is
  // unreadable is not evidence.
  console.log(`no purchase UI in ${Object.keys(files).length} shipped file(s)`)
  console.log(
    `artifact: ${STAMP_ATTR}="${buildVariant(files)}" under ${relative(root, dist)} — the bundle the iOS and Android shells package`,
  )
  console.log(`billing_policy.toml denies purchase UI, prices and locked-message CTAs on ${MOBILE.join(' and ')}`)
}
