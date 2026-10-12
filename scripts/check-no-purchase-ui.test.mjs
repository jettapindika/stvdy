/**
 * The purchase-UI check's own tests.
 *
 * Run with `node --test scripts/check-no-purchase-ui.test.mjs`. It is not part
 * of the vitest suite: that suite is `app/src/**` and this is a repo-root script
 * that reads the built output, which is the same split `check-no-cdn.mjs` has.
 *
 * Every case below is a case the check would get wrong in one of the two
 * directions that matter. A gate that cannot fail is worse than no gate, so the
 * offenders are planted; a gate that fails on the correct build gets deleted, so
 * the strings the app legitimately ships are pinned as non-offenders too.
 *
 * The second group pins the *identity* half, which is the half that decides
 * whether the byte scan meant anything. A bundle can be clean and still be the
 * wrong bundle — that is the state this check was written through, when the bytes
 * it read were the web build's — so "which artifact is this" gets its own cases:
 * a store artifact passes, a web/desktop artifact is refused, an unidentified
 * artifact is refused, and a store artifact that carries the purchase path is
 * still refused.
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { artifactOffenders, buildVariant, findOffenders, policyOffenders } from './check-no-purchase-ui.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

/** An `index.html` carrying the build's own variant stamp, as Vite emits it. */
function stamped(variant) {
  return `<!doctype html><html><head><meta charset="UTF-8"><meta name="stvdy-build-target" content="${variant}"></head><body><div id="root"></div></body></html>`
}

test('a bundle that carries a price is rejected', () => {
  const files = { 'assets/app.js': 'const price = "Rp 10.000"' }
  const offenders = findOffenders(files)
  assert.ok(offenders.length > 0, 'Rp must be caught')
})

test('a clean bundle passes', () => {
  assert.deepEqual(findOffenders({ 'assets/app.js': 'export const x = 1' }), [])
})

test('an empty bundle is rejected rather than passed', () => {
  // The whole point of the check is the one property a store rejection turns
  // on. A directory that was never built, or a build that emitted nothing this
  // check understands, must not read as "no purchase UI found".
  const offenders = findOffenders({})
  assert.equal(offenders.length, 1)
  assert.match(offenders[0], /no shipped files were inspected/)
})

test('a checkout route or endpoint is rejected', () => {
  assert.ok(findOffenders({ 'assets/api.js': "api.post('/billing/checkout', body)" }).length > 0)
})

test('a call to a purchase function is rejected', () => {
  assert.ok(findOffenders({ 'assets/app.js': 'await checkout(plan)' }).length > 0)
})

test('the strings the app legitimately ships are not offenders', () => {
  // Each of these is in the current `app/dist` build, and none of them is a
  // purchase path. If this test starts failing, the check has been widened into
  // a word search and will fail on the correct build.
  const shipped = {
    // `lock.upgrade`, in the en locale chunk. Data: the mobile policy never
    // renders it (LockedFeature reads `showUpgradeCta`, false for ios/android).
    'assets/index.js': 'lock:{notOnPlan:`Not available on your plan`,upgrade:`Upgrade to {{plan}}`}',
    // The styleguide's description of the two LockedFeature variants.
    'assets/index.js#2': 'Web and desktop name the plan and link to checkout; iOS and Android state the fact',
    // `config/plans.toml`, inlined whole by `plans.ts`'s `?raw` import.
    'assets/index.js#3': 'price_idr_monthly = 10000\n{ credits = 100, price_idr = 3000 }',
    // `config/billing_policy.toml`, inlined whole by `policy.ts`'s `?raw` import.
    'assets/index.js#4': '# QRIS checkout is web/desktop only, and the plan then syncs',
  }
  assert.deepEqual(findOffenders(shipped), [])
})

test('a policy that turns purchase UI on for a store platform is rejected', () => {
  const offenders = policyOffenders(`
[platforms.ios]
allow_purchase_ui = true
show_prices = false
allow_purchase_cta_in_locked_messages = false

[platforms.android]
allow_purchase_ui = false
show_prices = false
allow_purchase_cta_in_locked_messages = false
`)
  assert.equal(offenders.length, 1)
  assert.match(offenders[0], /\[platforms\.ios\] allow_purchase_ui = true/)
})

test('a policy that omits a store platform is rejected', () => {
  const offenders = policyOffenders(`
[platforms.android]
allow_purchase_ui = false
show_prices = false
allow_purchase_cta_in_locked_messages = false
`)
  assert.deepEqual(offenders, ['[platforms.ios] is missing from billing_policy.toml'])
})

test('the committed policy denies purchase UI on both store platforms', () => {
  const toml = readFileSync(join(root, 'config', 'billing_policy.toml'), 'utf8')
  assert.deepEqual(policyOffenders(toml), [])
})

test('the store artifact is the one this check accepts', () => {
  const files = { 'index.html': stamped('store'), 'assets/index.js': 'export const x = 1' }
  assert.equal(buildVariant(files), 'store')
  assert.deepEqual(artifactOffenders(files), [])
})

test('the web/desktop artifact is refused, however clean its bytes are', () => {
  // The defect this check was written through: `app/dist` held the web build, its
  // bytes were clean of nothing (a checkout route is legitimate there), and a green
  // result would have been read as "the phone build sells nothing". A clean `full`
  // artifact must still fail, because the property is about what a store receives.
  const files = { 'index.html': stamped('full'), 'assets/index.js': 'export const x = 1' }
  assert.deepEqual(findOffenders(files), [])
  const offenders = artifactOffenders(files)
  assert.equal(offenders.length, 1)
  assert.match(offenders[0], /"full"/)
})

test('an artifact with no stamp is refused rather than assumed clean', () => {
  // A directory named `dist-mobile` that nobody packages, or a build whose stamp
  // was dropped by a config change: an artifact the check cannot identify is one
  // whose cleanliness proves nothing, so silence is a failure.
  const files = { 'index.html': '<!doctype html><html><body></body></html>' }
  assert.equal(buildVariant(files), null)
  const offenders = artifactOffenders(files)
  assert.equal(offenders.length, 1)
  assert.match(offenders[0], /no stvdy-build-target stamp/)
})

test('an unrecognised variant is refused rather than assumed clean', () => {
  const files = { 'index.html': stamped('preview') }
  const offenders = artifactOffenders(files)
  assert.equal(offenders.length, 1)
  assert.match(offenders[0], /"preview"/)
})

test('a store stamp does not excuse a purchase path in the bytes', () => {
  // The two halves have to compose: the stamp says which artifact this is, and the
  // denylist says what it carries. A store artifact that still holds the checkout
  // route is a violation, and it must fail on the route rather than pass on the
  // stamp — otherwise stamping a build would be a way to skip the scan.
  const files = {
    'index.html': stamped('store'),
    'assets/index.js': "api.post('/billing/checkout', body)",
  }
  assert.deepEqual(artifactOffenders(files), [])
  const offenders = findOffenders(files)
  assert.equal(offenders.length, 1)
  assert.match(offenders[0], /billing\/checkout/)
})

test('an empty artifact is left to the denylist rather than reported twice', () => {
  // "Nothing was inspected" is `findOffenders`' message. A second copy of it here
  // would be a second answer to one question, and the check prints both.
  assert.deepEqual(artifactOffenders({}), [])
  assert.equal(findOffenders({}).length, 1)
})
