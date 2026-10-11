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
 * The codes are enumerated rather than left as `string` so a screen that forgets
 * to handle one cannot compile.
 */

export const API_ERROR_CODES = [
  'validation',
  'unauthorized',
  'token_expired',
  'plan_required',
  'feature_locked',
  'not_found',
  'conflict',
  'limit_reached',
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

  constructor(init: {
    status: number
    code: ApiErrorCode
    message: string
    minPlan?: string | null
    limit?: number | null
    used?: number | null
  }) {
    super(init.message)
    this.name = 'ApiError'
    this.status = init.status
    this.code = init.code
    this.minPlan = init.minPlan ?? null
    this.limit = init.limit ?? null
    this.used = init.used ?? null
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
    case 403:
      return 'plan_required'
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
 * None of these repeat the server's `message`, which is a log line. Each one says
 * what happened and what can be done about it, because "Something went wrong" is
 * a state that names neither (R-27).
 */
const REASON_BY_CODE: Record<ApiErrorCode, string> = {
  validation: 'The server did not accept one of these values. Correct it and try again.',
  unauthorized: 'Your session has ended. Sign in again to continue.',
  token_expired: 'Your session has ended. Sign in again to continue.',
  plan_required: 'This is not part of your current plan.',
  feature_locked: 'This is not part of your current plan.',
  not_found: 'It is no longer there. It may have been deleted, or it belongs to another account.',
  conflict: 'Something else changed this first. Reload and try again.',
  limit_reached: 'Your plan has reached its limit for this.',
  rate_limited: 'Too many attempts in a row. Wait a minute and try again.',
  provider_error: 'A service this depends on is failing. Nothing was saved. Try again shortly.',
  internal: 'The server failed on this request. Nothing was saved. Try again shortly.',
  network: 'The server could not be reached. Check your connection and try again.',
}

export function apiErrorReason(error: ApiError): string {
  return REASON_BY_CODE[error.code]
}

/** The heading that goes with each code. Same contract as the reasons above. */
const TITLE_BY_CODE: Record<ApiErrorCode, string> = {
  validation: 'That was not accepted',
  unauthorized: 'Signed out',
  token_expired: 'Signed out',
  plan_required: 'Not on your plan',
  feature_locked: 'Not on your plan',
  not_found: 'Not found',
  conflict: 'Changed elsewhere',
  limit_reached: 'Plan limit reached',
  rate_limited: 'Too many attempts',
  provider_error: 'Service unavailable',
  internal: 'Server error',
  network: 'No connection',
}

export function apiErrorTitle(error: ApiError): string {
  return TITLE_BY_CODE[error.code]
}

/** A plan id as the user sees it. `min_plan` arrives as the id the config uses. */
export function planLabel(planId: string): string {
  const KNOWN_PLANS: Record<string, string> = { free: 'Free', pro: 'Pro', power: 'Power' }
  return KNOWN_PLANS[planId] ?? planId
}

/**
 * The precise upgrade sentence for an entitlement refusal.
 *
 * The point of this function is that the prompt names the *numbers*, not just the
 * plan: "Free allows 2 workspaces and you have 2, Pro lifts this" is actionable,
 * and "upgrade to continue" is not. `limit` and `used` are only printed when the
 * server actually sent them, since a fabricated count is worse than a vaguer
 * sentence (R-17).
 *
 * `noun` is what the limit counts ("workspaces", "sources", "MB per file"), and
 * the caller supplies it because the envelope does not carry it and the call site
 * always knows it.
 */
export function planLimitMessage(error: ApiError, noun?: string): string {
  const thing = noun ?? 'this'
  const Thing = thing.charAt(0).toUpperCase() + thing.slice(1)
  const plan = error.minPlan === null ? null : planLabel(error.minPlan)

  const counts =
    error.limit !== null && error.used !== null
      ? `The limit is ${String(error.limit)} and you have ${String(error.used)}.`
      : error.limit !== null
        ? `The limit is ${String(error.limit)}.`
        : null

  if (error.code === 'feature_locked') {
    return plan === null
      ? `${Thing} is not included in your current plan.`
      : `${Thing} is not included in your current plan. It is part of ${plan}.`
  }

  const subject = plan === null ? 'Your plan' : `The ${plan} plan`
  const upgrade = plan === null ? null : `${plan} lifts this limit.`

  return [`${subject} has reached its limit for ${thing}.`, counts, upgrade]
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
 */
export function validationMessage(
  error: ApiError,
  candidates: readonly string[],
): { field: string | null; message: string } {
  const message = error.message.length > 0 ? error.message : apiErrorReason(error)
  const sentence = message.toLowerCase()

  const field =
    candidates.length === 1
      ? (candidates[0] ?? null)
      : (candidates.find((candidate) => sentence.includes(candidate.toLowerCase())) ?? null)

  return { field, message }
}
