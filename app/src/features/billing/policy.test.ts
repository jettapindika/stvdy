import { describe, expect, it } from 'vitest'

import { DENY, lockedFeaturePolicy, parsePolicy, policyFor } from './policy'

/**
 * The client half of the mobile purchase rule.
 *
 * `crates/core` validates the file on the server, and `crates/server` refuses
 * checkout for `ios`/`android`. These tests cover the third surface: the app must
 * not *contain* a purchase affordance on a phone build, because "the server would
 * have refused" is not a defence in an app-store review.
 */
describe('billing policy', () => {
  it('sells on web and desktop, and never on mobile', () => {
    for (const kind of ['web', 'macos', 'windows'] as const) {
      expect(policyFor(kind).allowPurchaseUi, kind).toBe(true)
      expect(policyFor(kind).showPrices, kind).toBe(true)
      expect(lockedFeaturePolicy(kind).showUpgradeCta, kind).toBe(true)
    }
    for (const kind of ['ios', 'android'] as const) {
      const policy = policyFor(kind)
      expect(policy.allowPurchaseUi, kind).toBe(false)
      expect(policy.showPrices, kind).toBe(false)
      expect(policy.allowPurchaseCtaInLockedMessages, kind).toBe(false)
      // The one that matters on a phone: no price, no button, no link.
      expect(lockedFeaturePolicy(kind), kind).toEqual({
        purchaseAllowed: false,
        showPrices: false,
        showUpgradeCta: false,
      })
    }
  })

  it('denies by default for an unknown platform', () => {
    // `linux` is out of scope for the MVP; if it ever appears, it must arrive
    // without purchase UI rather than inheriting it from a neighbour.
    expect(policyFor('linux' as never)).toEqual(DENY)
  })

  it('denies everything when the file cannot be parsed', () => {
    expect(parsePolicy('')).toEqual({})
    // A section header with an unparseable value keeps the section but leaves
    // every flag denied: the failure direction is the safe one.
    expect(parsePolicy('[platforms.ios]\nallow_purchase_ui = ')).toEqual({
      ios: { ...DENY },
    })
    // A typo in the value must not read as enabled, and an unknown key must not
    // invent a field.
    expect(parsePolicy('[platforms.ios]\nallow_purchase_ui = yes')).toEqual({
      ios: { ...DENY },
    })
    expect(parsePolicy('[platforms.ios]\nsell_everything = true')).toEqual({
      ios: { ...DENY },
    })
  })

  it('keeps a value with the section it belongs to', () => {
    const parsed = parsePolicy(
      [
        '# a comment mentioning [platforms.macos]',
        '[platforms.web]',
        'allow_purchase_ui = true',
        'show_prices = true',
        'allow_purchase_cta_in_locked_messages = true',
        '',
        '[platforms.android]',
        'allow_purchase_ui = false',
      ].join('\n'),
    )
    expect(parsed.web).toEqual({
      allowPurchaseUi: true,
      showPrices: true,
      allowPurchaseCtaInLockedMessages: true,
    })
    expect(parsed.android).toEqual({ ...DENY })
  })

  it('reads the shipped file, not a copy of it', () => {
    // If `config/billing_policy.toml` were edited to enable purchase UI on a
    // phone, this test would fail rather than the app shipping a store
    // violation. The Rust side has the same guard.
    const shipped = policyFor('ios')
    expect(shipped.allowPurchaseUi).toBe(false)
    expect(Object.keys(parsePolicy('')).length).toBe(0)
  })
})
