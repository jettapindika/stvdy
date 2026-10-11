import policyToml from '@config/billing_policy.toml?raw'

import type { PlatformKind } from '@/platform'

/**
 * Where purchase UI is allowed, read from `config/billing_policy.toml`.
 *
 * The file is the single source of truth and the Rust side already refuses
 * checkout for `ios`/`android` (A24). This module is the client half of the same
 * rule, and it is deliberately *not* the enforcement: a patched client that flips
 * a flag here still gets a 403 from `POST /billing/checkout`. What it buys is that
 * the phone builds contain no checkout screen, no price and no upgrade button to
 * begin with, which is the thing the app stores review.
 *
 * The TOML is imported as text at build time rather than duplicated as
 * TypeScript, so the policy cannot drift between the server and the app. The
 * parser below is intentionally tiny and strict: it understands the one shape
 * this file has, and anything it does not understand falls through to
 * `DENY`, which is the same deny-by-default rule the Rust loader uses.
 */
export interface PlatformPolicy {
  allowPurchaseUi: boolean
  showPrices: boolean
  allowPurchaseCtaInLockedMessages: boolean
}

/**
 * The policy when a platform key is missing or unparseable.
 *
 * Deny, always. A parse failure that enabled selling on mobile would be a policy
 * violation produced by a typo, so the failure direction is the safe one.
 */
export const DENY: PlatformPolicy = {
  allowPurchaseUi: false,
  showPrices: false,
  allowPurchaseCtaInLockedMessages: false,
}

/** Parses the `[platforms.<name>]` blocks of the policy file. */
export function parsePolicy(toml: string): Partial<Record<string, PlatformPolicy>> {
  const policies: Partial<Record<string, PlatformPolicy>> = {}
  let current: PlatformPolicy | null = null

  for (const rawLine of toml.split('\n')) {
    // Comments are stripped before anything else, so a `#` inside a value cannot
    // be mistaken for structure. This file has no quoted values, so there is no
    // string-literal case to handle.
    const line = (rawLine.split('#')[0] ?? '').trim()
    if (line === '') continue

    const section = /^\[platforms\.([a-z]+)\]$/.exec(line)
    const platform = section?.[1]
    if (platform) {
      current = { ...DENY }
      policies[platform] = current
      continue
    }
    if (line.startsWith('[')) {
      // An unexpected section ends the block rather than being ignored: a value
      // after it would otherwise be attributed to the wrong platform.
      current = null
      continue
    }

    const entry = /^([a-z_]+)\s*=\s*(true|false)$/.exec(line)
    const key = entry?.[1]
    if (!key || !current) continue
    const on = entry[2] === 'true'
    if (key === 'allow_purchase_ui') current.allowPurchaseUi = on
    if (key === 'show_prices') current.showPrices = on
    if (key === 'allow_purchase_cta_in_locked_messages') current.allowPurchaseCtaInLockedMessages = on
  }

  return policies
}

const POLICIES = parsePolicy(policyToml)

/** The policy for a platform, defaulting to `DENY`. */
export function policyFor(kind: PlatformKind): PlatformPolicy {
  return POLICIES[kind] ?? DENY
}

/**
 * What a locked feature may show on this platform.
 *
 * `purchaseAllowed` is the one flag screens branch on; the other two exist so a
 * future screen can show a price without a button, or a button without a price.
 */
export function lockedFeaturePolicy(kind: PlatformKind): {
  purchaseAllowed: boolean
  showPrices: boolean
  showUpgradeCta: boolean
} {
  const policy = policyFor(kind)
  return {
    purchaseAllowed: policy.allowPurchaseUi,
    showPrices: policy.showPrices,
    showUpgradeCta: policy.allowPurchaseUi && policy.allowPurchaseCtaInLockedMessages,
  }
}
