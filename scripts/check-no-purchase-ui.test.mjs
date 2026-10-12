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
 */
import assert from 'node:assert/strict'
import { readFileSync } from 'node:fs'
import { dirname, join } from 'node:path'
import { test } from 'node:test'
import { fileURLToPath } from 'node:url'

import { findOffenders, policyOffenders } from './check-no-purchase-ui.mjs'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

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
