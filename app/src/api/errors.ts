/**
 * The one error shape every call can produce.
 *
 * The server's envelope is fixed: `{"code","message","min_plan","limit","used"}`.
 * `code` is the stable, machine-readable half and `message` is written for logs,
 * so screens branch on `code` and never on a message string. That is why this
 * module owns the *user-facing* sentence for each code as well: the mapping is
 * exhaustive over `ApiErrorCode`, so adding a code without deciding what the user
 * is told is a type error rather than a blank error box.
 *
 * **The sentences are translation keys, resolved through the `t` every accessor
 * takes.** They used to be literals in this file, which is the one place the i18n
 * contract structurally could not see: `i18n.test.ts` scans components for
 * `t('…')` calls, so copy held in a `.ts` table passed every check while an
 * Indonesian user read English refusals. Taking `t` rather than importing the
 * i18n singleton keeps this module free of the app's i18n instance and lets a
 * screen pass the reactive `t` it already has, so a language change re-renders
 * the notice instead of freezing whichever language was current when the error
 * was raised.
 *
 * The codes are enumerated rather than left as `string` so a screen that forgets
 * to handle one cannot compile.
 */

import type { TFunction } from 'i18next'

export const API_ERROR_CODES = [
  'validation',
  'unauthorized',
  'token_expired',
  'plan_required',
  'feature_locked',
  /**
   * A refusal with no plan story behind it: a signature that does not match
   * (A73), a device over its allowance, a caller not allowed to read this row.
   *
   * It is also what a 403 becomes when the body carries no code this build
   * knows. That fallback is the point of the arm: the notice for `plan_required`
   * offers "See plans", and a plan cannot fix a tampered signature or an
   * unverified email — which is how a deleted account came to be rendered as a
   * plan gate, before A362 made a deleted account unrepresentable here.
   */
  'forbidden',
  /** A verified email is required before any action that costs money (A43). */
  'email_not_verified',
  'not_found',
  'conflict',
  'limit_reached',
  /**
   * A signed URL past its expiry. Distinct from `unauthorized`: the remedy is to
   * fetch a fresh link, not to sign in again.
   */
  'link_expired',
  /**
   * The sync cursor is older than the history the server still retains, so this
   * device cannot catch up incrementally and has to start over (A186). Distinct
   * from `link_expired`, which shares its status: "open the source again" is the
   * wrong instruction for a copy that is too far behind.
   */
  'resync_required',
  /** The account's credit balance is short of what the action costs. */
  'insufficient_credits',
  'rate_limited',
  'provider_error',
  'internal',
  /** Client-side only: the request never reached the server. */
  'network',
] as const

export type ApiErrorCode = (typeof API_ERROR_CODES)[number]

/** The raw envelope, exactly as the server writes it. */
export interface ApiErrorBody {
  code: string
  message: string
  min_plan: string | null
  limit: number | null
  used: number | null
}

export class ApiError extends Error {
  readonly status: number
  readonly code: ApiErrorCode
  /** The plan that would lift the limit. Present on entitlement errors. */
  readonly minPlan: string | null
  /** The plan's ceiling for the thing that was refused. */
  readonly limit: number | null
  /** What the account has already used of it. */
  readonly used: number | null
  /**
   * The API origin a request never reached. Set only on the client's own
   * `network` errors, which are raised before any server answered.
   *
   * The origin is the actionable half of that notice: "the server could not be
   * reached" is the same sentence for three different situations (the API is not
   * running, the machine has no network, the build points at an origin nothing
   * serves any more), and only the origin tells them apart. It is carried as data
   * rather than baked into `message` so the sentence is resolved by
   * `apiErrorReason` in the language the user is reading, not the one that was
   * current when the request failed.
   */
  readonly origin: string | null
  /**
   * A translation key for a condition the *client* distinguishes and the server's
   * code set does not.
   *
   * `network` covers two situations with different sentences: the request never
   * reached the server (a dropped connection, an API that is not running) and the
   * connection dropped *mid-answer*, after the server had answered. Saying "the
   * server could not be reached" for the second is untrue — it was reached — so
   * the raiser names the key, and `apiErrorReason` resolves it in the reader's
   * language. It is a key rather than a sentence for the same reason the rest of
   * the copy is: a literal here is English shown to an Indonesian user.
   */
  readonly reasonKey: string | null

  constructor(init: {
    status: number
    code: ApiErrorCode
    message: string
    minPlan?: string | null
    limit?: number | null
    used?: number | null
    origin?: string | null
    reasonKey?: string | null
  }) {
    super(init.message)
    this.name = 'ApiError'
    this.status = init.status
    this.code = init.code
    this.minPlan = init.minPlan ?? null
    this.limit = init.limit ?? null
    this.used = init.used ?? null
    this.origin = init.origin ?? null
    this.reasonKey = init.reasonKey ?? null
  }

  /** True when the session is over and the app must show the signed-out state. */
  get isAuthFailure(): boolean {
    return this.code === 'unauthorized' || this.code === 'token_expired'
  }

  /** True when the fix is a different plan, not a different input. */
  get isPlanLimit(): boolean {
    return (
      this.code === 'limit_reached' ||
      this.code === 'plan_required' ||
      this.code === 'feature_locked'
    )
  }
}

function isApiErrorCode(value: string): value is ApiErrorCode {
  return (API_ERROR_CODES as readonly string[]).includes(value)
}

/** Maps an HTTP status to a code, for a response whose body is missing or junk. */
export function codeForStatus(status: number): ApiErrorCode {
  switch (status) {
    case 400:
      return 'validation'
    case 401:
      return 'unauthorized'
    case 410:
      return 'link_expired'
    case 402:
      return 'insufficient_credits'
    case 403:
      // 403 is the overloaded status — a plan gate, a locked feature, an
      // unverified email, a tampered signature, a device over its allowance — and
      // the body's `code` is what tells them apart. This arm is only for a body
      // that carried none, so it has to be the one that claims nothing:
      // `plan_required` here offered "See plans" as the remedy for every refusal
      // whose code this build did not recognise.
      return 'forbidden'
    case 404:
      return 'not_found'
    case 409:
      return 'conflict'
    case 422:
      return 'limit_reached'
    case 429:
      return 'rate_limited'
    case 502:
      return 'provider_error'
    default:
      return status >= 500 ? 'internal' : 'validation'
  }
}

/**
 * The status a code would have arrived with, for a body that never had one.
 *
 * The SSE `error` event carries the same five-key envelope as every other route
 * but no HTTP status: it is a frame inside a 200 response. Reconstructing the
 * status here keeps one error type across the whole client instead of a second
 * one for streams, and it stays honest because this is the exact inverse of
 * `codeForStatus` for the codes a stream can carry.
 */
export function statusForCode(code: ApiErrorCode): number {
  switch (code) {
    case 'insufficient_credits':
      return 402
    case 'forbidden':
    case 'email_not_verified':
    case 'plan_required':
    case 'feature_locked':
      return 403
    case 'not_found':
      return 404
    case 'conflict':
      return 409
    case 'limit_reached':
      return 422
    case 'link_expired':
    case 'resync_required':
      return 410
    case 'rate_limited':
      return 429
    case 'unauthorized':
    case 'token_expired':
      return 401
    case 'provider_error':
      return 502
    case 'internal':
      return 500
    /** Client-side only: the request never reached the server, so there is no status. */
    case 'network':
      return 0
    default:
      return 400
  }
}

/**
 * Turns a response body into an `ApiError`.
 *
 * A proxy, a crash before the error middleware, or an empty body all produce
 * something that is not the documented envelope. Those still have to become a
 * typed error, because a screen cannot render `undefined`, so the status decides
 * the code and the message is left empty for the caller to replace with its own
 * sentence.
 */
export function apiErrorFromBody(status: number, body: unknown): ApiError {
  const envelope = typeof body === 'object' && body !== null ? (body as Record<string, unknown>) : {}

  const rawCode = typeof envelope.code === 'string' ? envelope.code : null
  const code = rawCode !== null && isApiErrorCode(rawCode) ? rawCode : codeForStatus(status)

  return new ApiError({
    status,
    code,
    message: typeof envelope.message === 'string' ? envelope.message : '',
    minPlan: typeof envelope.min_plan === 'string' ? envelope.min_plan : null,
    limit: typeof envelope.limit === 'number' ? envelope.limit : null,
    used: typeof envelope.used === 'number' ? envelope.used : null,
  })
}

/**
 * What the user is told, per code.
 *
 * These do not repeat the server's `message`, which is a log line. Each one says
 * what happened and what can be done about it, because "Something went wrong" is
 * a state that names neither (R-27). The copy lives in `error.<code>.reason` in
 * the locale files, so the mapping here is the *key* rather than the sentence and
 * an Indonesian user reads Indonesian.
 *
 * `validation` is the one exception: its server sentence is the only statement of
 * which field was rejected, and it is handled by `validationMessage`.
 */
export function apiErrorReason(error: ApiError, t: TFunction): string {
  // `network` is the only code this client raises itself, and it is the only one
  // whose sentence depends on something the envelope cannot carry: which origin
  // was tried, or which of the two drops it was.
  if (error.code === 'network') {
    if (error.reasonKey !== null) return t(error.reasonKey)
    return error.origin === null
      ? t('error.network.reason')
      : t('error.network.reasonAt', { origin: error.origin })
  }

  return t(`error.${error.code}.reason`)
}

/** The heading that goes with each code. Same contract as the reasons above. */
export function apiErrorTitle(error: ApiError, t: TFunction): string {
  return t(`error.${error.code}.title`)
}

/**
 * A plan id as the user sees it. `min_plan` arrives as the id the config uses.
 *
 * The three names are proper nouns and are the ones `config/plans.toml` declares
 * under `[plans.<id>] name`, which is also what `LockedFeature`'s `minPlan` prop
 * spells. They are not translated: "Pro" is the product's name for the plan in
 * both languages, and a locale file that renamed it would make the locked copy
 * disagree with the plan screen's own heading.
 */
export function planLabel(planId: string): string {
  const KNOWN_PLANS: Record<string, string> = { free: 'Free', pro: 'Pro', power: 'Power' }
  return KNOWN_PLANS[planId] ?? planId
}

/**
 * The precise sentence for an entitlement refusal.
 *
 * The point of this function is that the prompt names the *numbers*, not just the
 * plan: "Free allows 2 workspaces and you have 2, Pro lifts this" is actionable,
 * and "upgrade to continue" is not. `limit` and `used` are only printed when the
 * server actually sent them, since a fabricated count is worse than a vaguer
 * sentence (R-17).
 *
 * `noun` is what the limit counts ("workspaces", "sources", "MB per file"), and
 * the caller supplies it because the envelope does not carry it and the call site
 * always knows it. It arrives already translated, from `errors.nouns.*`.
 *
 * **The three codes are three different facts and get three different sentences**,
 * from `error.limit.*` and `error.locked.*` keys rather than from English
 * fragments — this is the copy a refused user actually reads, so it is the one
 * place an untranslated string is a refusal in the wrong language:
 *
 * - `limit_reached` — the account is at a ceiling. Names the plan and the numbers.
 * - `plan_required` — the feature is not in this plan at all. Names the plan when
 *   the server sent one, because a higher plan includes it and the notice beside
 *   this sentence links to the plans screen.
 * - `feature_locked` — locked, and no upgrade may be offered to this client
 *   (A143): the platform may not sell, or no plan grants the feature. It names no
 *   plan and no price, and it does not claim a limit was reached, because the code
 *   also covers a device that is over its plan's device count — where the plan is
 *   not the problem and the fix is to sign another device out.
 */
export function planLimitMessage(error: ApiError, t: TFunction, noun?: string): string {
  const thing = noun ?? 'this'
  const plan = error.minPlan === null ? null : planLabel(error.minPlan)
  // The feature name leads each sentence, so it is capitalised the way a sentence
  // starts. The capital is added here rather than baked into the locale string
  // because the value is a translated noun the caller chose.
  const feature = thing.charAt(0).toUpperCase() + thing.slice(1)

  if (error.code === 'feature_locked') {
    return t('error.locked.unavailable', { feature })
  }

  if (error.code === 'plan_required') {
    return plan === null
      ? t('error.locked.notIncluded', { feature })
      : t('error.locked.notIncludedIn', { feature, plan })
  }

  const counts =
    error.limit !== null && error.used !== null
      ? t('error.limit.countsBoth', { limit: error.limit, used: error.used })
      : error.limit !== null
        ? t('error.limit.count', { limit: error.limit })
        : null

  const subject = plan === null ? t('error.limit.yourPlan') : t('error.limit.thePlan', { plan })
  const upgrade = plan === null ? null : t('error.limit.lifts', { plan })

  return [t('error.limit.reached', { subject, noun: thing }), counts, upgrade]
    .filter((part): part is string => part !== null)
    .join(' ')
}

/**
 * Anything thrown becomes an `ApiError`.
 *
 * The client already wraps every failure, so this exists for the case where it
 * did not: a bug in a call site should still render an honest error state rather
 * than a silently empty form.
 */
export function asApiError(error: unknown): ApiError | null {
  if (error === null || error === undefined) return null
  if (error instanceof ApiError) return error
  return new ApiError({ status: 0, code: 'internal', message: '' })
}

/**
 * Where a `validation` failure is shown, and what it says.
 *
 * The envelope carries no field map, so the code and the server's sentence are
 * the only signals. With exactly one candidate the implication is unambiguous.
 * With several, the sentence is checked for a candidate's name ("password must be
 * at least 12 characters" belongs on the password field), and when nothing
 * matches, the message goes to the top of the form rather than being pinned to a
 * guessed field, where it would send the user to correct something that was
 * never wrong.
 *
 * Matching on the sentence is safe here precisely because the fallback is not a
 * failure: a reworded message moves the notice from a field to the form, which is
 * a worse position but never a wrong one. Nothing about *behaviour* depends on
 * the text.
 *
 * The server's `message` is used here and nowhere else. It is written for logs,
 * but on a validation failure it is the only statement of what was rejected, and
 * a form that says only "one of these is wrong" is not usable.
 *
 * `t` is only consulted for the fallback, when the server sent no sentence: that
 * fallback is the `validation` reason from the locale files, so a form whose
 * failure had no message still says something in the user's language.
 */
export function validationMessage(
  error: ApiError,
  t: TFunction,
  candidates: readonly string[],
): { field: string | null; message: string } {
  const message = error.message.length > 0 ? error.message : apiErrorReason(error, t)
  const sentence = message.toLowerCase()

  const field =
    candidates.length === 1
      ? (candidates[0] ?? null)
      : (candidates.find((candidate) => sentence.includes(candidate.toLowerCase())) ?? null)

  return { field, message }
}
