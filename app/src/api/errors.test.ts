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
    expect(codeForStatus(403)).toBe('plan_required')
    expect(codeForStatus(404)).toBe('not_found')
    expect(codeForStatus(409)).toBe('conflict')
    expect(codeForStatus(422)).toBe('limit_reached')
    expect(codeForStatus(429)).toBe('rate_limited')
    expect(codeForStatus(502)).toBe('provider_error')
    expect(codeForStatus(500)).toBe('internal')
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
      'not_found',
      'conflict',
      'limit_reached',
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
