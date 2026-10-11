import { describe, expect, it } from 'vitest'

import {
  ApiError,
  apiErrorReason,
  apiErrorTitle,
  apiErrorFromBody,
  asApiError,
  codeForStatus,
  planLabel,
  planLimitMessage,
  statusForCode,
  validationMessage,
} from './errors'

/**
 * The error vocabulary.
 *
 * These tests pin the two things a screen depends on: that a server body becomes
 * the right typed error, and that the sentence the user reads is built from the
 * numbers the server actually sent rather than from a guess.
 */

describe('mapping a response body', () => {
  it('keeps every documented field', () => {
    const error = apiErrorFromBody(422, {
      code: 'limit_reached',
      message: 'workspaces limit reached',
      min_plan: 'pro',
      limit: 2,
      used: 2,
    })

    expect(error).toBeInstanceOf(ApiError)
    expect(error.status).toBe(422)
    expect(error.code).toBe('limit_reached')
    expect(error.minPlan).toBe('pro')
    expect(error.limit).toBe(2)
    expect(error.used).toBe(2)
    expect(error.isPlanLimit).toBe(true)
    expect(error.isAuthFailure).toBe(false)
  })

  it('derives the code from the status when the body is not the envelope', () => {
    expect(apiErrorFromBody(500, null).code).toBe('internal')
    expect(apiErrorFromBody(429, 'slow down').code).toBe('rate_limited')
    // An unrecognised code is not passed through as a string: a screen can then
    // always answer "the server failed", which is true for anything unknown.
    expect(apiErrorFromBody(418, { code: 'teapot' }).code).toBe('validation')
  })

  it('flags the two codes that end a session', () => {
    expect(apiErrorFromBody(401, { code: 'unauthorized' }).isAuthFailure).toBe(true)
    expect(apiErrorFromBody(401, { code: 'token_expired' }).isAuthFailure).toBe(true)
    expect(apiErrorFromBody(403, { code: 'plan_required' }).isAuthFailure).toBe(false)
  })

  it('maps the documented statuses', () => {
    expect(codeForStatus(400)).toBe('validation')
    expect(codeForStatus(401)).toBe('unauthorized')
    expect(codeForStatus(402)).toBe('insufficient_credits')
    expect(codeForStatus(403)).toBe('plan_required')
    expect(codeForStatus(404)).toBe('not_found')
    expect(codeForStatus(409)).toBe('conflict')
    expect(codeForStatus(422)).toBe('limit_reached')
    expect(codeForStatus(429)).toBe('rate_limited')
    expect(codeForStatus(502)).toBe('provider_error')
    expect(codeForStatus(500)).toBe('internal')
  })

  it('reconstructs the status a streamed error would have arrived with', () => {
    // The SSE `error` event carries the same envelope but no status, so the code
    // is the only signal. This is the inverse of `codeForStatus`, and it has to
    // stay the inverse or a streamed refusal would render with the wrong
    // treatment (an upgrade prompt for a wrong password, say).
    expect(statusForCode('insufficient_credits')).toBe(402)
    expect(statusForCode('email_not_verified')).toBe(403)
    expect(statusForCode('plan_required')).toBe(403)
    expect(statusForCode('feature_locked')).toBe(403)
    expect(statusForCode('not_found')).toBe(404)
    expect(statusForCode('conflict')).toBe(409)
    expect(statusForCode('limit_reached')).toBe(422)
    expect(statusForCode('rate_limited')).toBe(429)
    expect(statusForCode('unauthorized')).toBe(401)
    expect(statusForCode('provider_error')).toBe(502)
    expect(statusForCode('internal')).toBe(500)
    // Client-side only: there is no status to reconstruct.
    expect(statusForCode('network')).toBe(0)
  })

  it('does not treat a spent-out account as a plan limit', () => {
    const credits = apiErrorFromBody(402, { code: 'insufficient_credits' })
    expect(credits.isPlanLimit).toBe(false)
    expect(credits.status).toBe(402)

    // Nor an unverified one: the remedy is an inbox, not an upgrade.
    const unverified = apiErrorFromBody(403, { code: 'email_not_verified' })
    expect(unverified.isPlanLimit).toBe(false)
    expect(unverified.isAuthFailure).toBe(false)
    expect(apiErrorTitle(unverified)).toBe('Email not verified')
  })

  it('turns an unknown throw into a renderable error', () => {
    expect(asApiError(null)).toBeNull()
    expect(asApiError(new Error('boom'))?.code).toBe('internal')
    const known = apiErrorFromBody(404, { code: 'not_found' })
    expect(asApiError(known)).toBe(known)
  })

  it('has a title and a reason for every code', () => {
    for (const code of [
      'validation',
      'unauthorized',
      'token_expired',
      'plan_required',
      'feature_locked',
      'email_not_verified',
      'not_found',
      'conflict',
      'limit_reached',
      'insufficient_credits',
      'rate_limited',
      'provider_error',
      'internal',
      'network',
    ] as const) {
      const error = new ApiError({ status: 0, code, message: '' })
      expect(apiErrorTitle(error).length).toBeGreaterThan(0)
      expect(apiErrorReason(error).length).toBeGreaterThan(0)
    }
  })
})

describe('the upgrade sentence', () => {
  it('names the plan, the limit, and what the account has used', () => {
    const error = apiErrorFromBody(422, {
      code: 'limit_reached',
      message: 'workspaces limit reached',
      min_plan: 'pro',
      limit: 2,
      used: 2,
    })

    const sentence = planLimitMessage(error, 'workspaces')
    expect(sentence).toContain('The Pro plan')
    expect(sentence).toContain('2')
    expect(sentence).toContain('Pro lifts this limit.')
  })

  it('does not invent a number the server did not send', () => {
    const error = apiErrorFromBody(422, { code: 'limit_reached', min_plan: 'power' })
    const sentence = planLimitMessage(error, 'sources')
    expect(sentence).toContain('The Power plan')
    expect(sentence).not.toContain('The limit is')
  })

  it('says which plan a locked feature belongs to', () => {
    const error = apiErrorFromBody(403, { code: 'feature_locked', min_plan: 'pro' })
    expect(planLimitMessage(error, 'web search')).toBe(
      'Web search is not included in your current plan. It is part of Pro.',
    )
  })

  it('prints a plan id the way the plans config names it', () => {
    expect(planLabel('free')).toBe('Free')
    expect(planLabel('pro')).toBe('Pro')
    expect(planLabel('power')).toBe('Power')
    expect(planLabel('enterprise')).toBe('enterprise')
  })
})

describe('validation attribution', () => {
  it('uses the only candidate when there is one', () => {
    const error = apiErrorFromBody(400, { code: 'validation', message: 'must not be blank' })
    expect(validationMessage(error, ['name'])).toEqual({
      field: 'name',
      message: 'must not be blank',
    })
  })

  it('picks the candidate the server named', () => {
    const error = apiErrorFromBody(400, {
      code: 'validation',
      message: 'password must be at least 12 characters',
    })
    expect(validationMessage(error, ['email', 'password', 'device']).field).toBe('password')
  })

  it('leaves the message at the form level when no field is identified', () => {
    const error = apiErrorFromBody(400, { code: 'validation', message: 'request rejected' })
    expect(validationMessage(error, ['email', 'password']).field).toBeNull()
  })

  it('falls back to a sentence of its own when the server sent none', () => {
    const error = apiErrorFromBody(400, { code: 'validation', message: '' })
    const { message } = validationMessage(error, ['email'])
    expect(message).toBe(apiErrorReason(error))
    expect(message.length).toBeGreaterThan(0)
  })
})
