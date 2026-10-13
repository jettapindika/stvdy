import { describe, expect, it } from 'vitest'

import {
  API_ERROR_CODES,
  ApiError,
  type ApiErrorCode,
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
import i18n from '@/i18n'

/**
 * A `t` for the accessors outside a component.
 *
 * `getFixedT(null)` follows the current language rather than pinning one, which
 * is what lets the Indonesian assertions below drive the same functions the
 * screens call. Screens use the reactive `t` from `useTranslation` instead: this
 * one is not a hook, so a language change would not re-render around it.
 */
const t = i18n.getFixedT(null, 'translation')

/**
 * The error vocabulary.
 *
 * These tests pin the three things a screen depends on: that a server body
 * becomes the right typed error, that the sentence the user reads is built from
 * the numbers the server actually sent rather than from a guess, and that every
 * one of those sentences exists in both languages. The last one is here rather
 * than in `i18n.test.ts` because the keys are built by interpolation
 * (`error.${code}.title`), which no static scan of the source can see: the
 * i18n contract would pass with a code whose copy is missing, and the user would
 * read the key itself.
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
    // Not `plan_required`: this arm answers a body that carried no code at all,
    // and 403 covers a tampered signature, an unverified email and a deleted
    // account as well as a plan gate. The body's `code` picks among them, and
    // guessing a plan here is what put "See plans" on a deleted account.
    expect(codeForStatus(403)).toBe('forbidden')
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
    expect(apiErrorTitle(unverified, t)).toBe('Email not verified')
  })

  it('turns an unknown throw into a renderable error', () => {
    expect(asApiError(null)).toBeNull()
    expect(asApiError(new Error('boom'))?.code).toBe('internal')
    const known = apiErrorFromBody(404, { code: 'not_found' })
    expect(asApiError(known)).toBe(known)
  })

  it('names the origin it could not reach, and keeps a sentence without one', () => {
    // A `network` error is raised by the client, not by the server. Its origin is
    // the actionable part: "the server could not be reached" is the same sentence
    // for three different situations (no API running, no network, a build pointed
    // at an origin nothing serves), and only the origin tells them apart. It is
    // carried as data rather than baked into the message, so the sentence is
    // resolved in the language the reader is in.
    const reached = new ApiError({ status: 0, code: 'network', message: '', origin: 'http://localhost:8080' })
    expect(apiErrorReason(reached, t)).toBe(
      'The server could not be reached at http://localhost:8080. Check your connection and try again.',
    )

    // An error built without an origin still reads as something, which is what
    // the exhaustive table above guarantees.
    const bare = new ApiError({ status: 0, code: 'network', message: '' })
    expect(apiErrorReason(bare, t)).toBe(
      'The server could not be reached. Check your connection and try again.',
    )
  })

  it('has a title and a reason for every code', () => {
    // Iterates the exported list rather than a copy of it. A hardcoded list
    // cannot fail when a code is added on the server, which is exactly how
    // `link_expired` shipped with no client mapping.
    for (const code of API_ERROR_CODES) {
      const error = new ApiError({ status: 0, code, message: '' })
      expect(apiErrorTitle(error, t).length).toBeGreaterThan(0)
      expect(apiErrorReason(error, t).length).toBeGreaterThan(0)
    }
  })

  it('gives every code its documented status', () => {
    // The switches are easy to leave half-wired: `Record<ApiErrorCode, …>` forces
    // the *tables* to be complete, but a missing `case` in a switch falls through
    // to a `default` arm and `tsc` stays green. That is how `link_expired` shipped
    // mapping to 400. This table is the documented status per code, and it is
    // checked for completeness against the exported list.
    //
    // A round trip is not usable here: several codes share a status (401 is both
    // `unauthorized` and `token_expired`), so `codeForStatus(statusForCode(code))`
    // legitimately returns a sibling.
    const expected: Record<ApiErrorCode, number> = {
      validation: 400,
      unauthorized: 401,
      token_expired: 401,
      plan_required: 403,
      feature_locked: 403,
      account_deleted: 403,
      forbidden: 403,
      email_not_verified: 403,
      not_found: 404,
      conflict: 409,
      link_expired: 410,
      resync_required: 410,
      limit_reached: 422,
      insufficient_credits: 402,
      rate_limited: 429,
      provider_error: 502,
      internal: 500,
      network: 0,
    }

    // Every exported code has an entry, and every entry is an exported code: a
    // code added on the server and not here fails, and a stale entry fails too.
    expect(Object.keys(expected).sort()).toEqual([...API_ERROR_CODES].sort())

    for (const code of API_ERROR_CODES) {
      expect({ code, status: statusForCode(code) }).toEqual({
        code,
        status: expected[code],
      })
    }

    // The statuses the server actually sends map back to a code, never to a
    // fall-through.
    for (const status of [400, 401, 402, 403, 404, 409, 410, 422, 429, 500, 502]) {
      expect(API_ERROR_CODES).toContain(codeForStatus(status))
    }
  })

  it('does not read a 403 with no usable code as a plan gate', () => {
    // The defect this pins: `codeForStatus(403)` used to answer `plan_required`,
    // and `account_deleted` was not in `API_ERROR_CODES` at all, so the refusal a
    // deleted account meets — 403, with the purge date in `message` — arrived as a
    // plan gate and the signup screen rendered "Not on your plan / This is not
    // included in your current plan" beside a link to the plans page. Nothing a
    // plan can fix. Both halves are asserted: an unknown code must not become a
    // plan limit, and the real code must be recognised rather than fallen back to.
    const unknown = apiErrorFromBody(403, { code: 'account_deleted_v2', message: 'gone' })
    expect(unknown.code).toBe('forbidden')
    expect(unknown.isPlanLimit).toBe(false)
    expect(apiErrorTitle(unknown, t)).toBe('Not allowed')

    const bodiless = apiErrorFromBody(403, null)
    expect(bodiless.code).toBe('forbidden')
    expect(bodiless.isPlanLimit).toBe(false)
  })

  it('shows the purge date a deleted account is refused with', () => {
    // The date exists only in the server's sentence, so `apiErrorReason` returns
    // it verbatim for this code. Composing one from the envelope is not possible:
    // no field carries it.
    const deleted = apiErrorFromBody(403, {
      code: 'account_deleted',
      message:
        'This account was deleted at your request; its data is removed on 2026-10-20 12:13 UTC, which releases this address for a new account',
      min_plan: null,
      limit: null,
      used: null,
    })

    expect(deleted.code).toBe('account_deleted')
    expect(deleted.isPlanLimit).toBe(false)
    expect(apiErrorTitle(deleted, t)).toBe('Account deleted')
    expect(apiErrorReason(deleted, t)).toContain('2026-10-20')
    expect(apiErrorReason(deleted, t)).not.toContain('error.')

    // A refusal with no sentence still reads as something rather than as a key.
    const silent = apiErrorFromBody(403, { code: 'account_deleted', message: '' })
    expect(apiErrorReason(silent, t)).toContain('deleted')
    expect(apiErrorReason(silent, t)).not.toContain('error.')
  })

  it('reads a 410 as an expired link rather than a validation failure', () => {
    // The server distinguishes an expired signed URL (410 `link_expired`) from a
    // tampered one (403), because the remedy differs. Falling through to
    // `validation` would tell the user the request was malformed.
    const expired = apiErrorFromBody(410, { code: 'link_expired', message: 'expired' })
    expect(expired.code).toBe('link_expired')
    expect(apiErrorReason(expired, t)).toContain('expired')

    // A 410 with a body the client does not recognise still reads as expiry.
    const bare = apiErrorFromBody(410, null)
    expect(bare.code).toBe('link_expired')
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

    const sentence = planLimitMessage(error, t, 'workspaces')
    expect(sentence).toContain('The Pro plan')
    expect(sentence).toContain('2')
    expect(sentence).toContain('Pro lifts this limit.')
  })

  it('does not invent a number the server did not send', () => {
    const error = apiErrorFromBody(422, { code: 'limit_reached', min_plan: 'power' })
    const sentence = planLimitMessage(error, t, 'sources')
    expect(sentence).toContain('The Power plan')
    expect(sentence).not.toContain('The limit is')
  })

  it('says which plan includes a feature the plan does not have', () => {
    const error = apiErrorFromBody(403, { code: 'plan_required', min_plan: 'pro' })
    expect(planLimitMessage(error, t, 'pdf export')).toBe(
      'Pdf export is not included in your current plan. It is part of Pro.',
    )
  })

  it('states a plan-required feature without naming a plan the server did not send', () => {
    // `min_plan` is optional on `plan_required` (A140): a feature no plan in the
    // matrix grants has no plan to name, and a fabricated one would send the user
    // to a plans page that cannot fix it.
    const error = apiErrorFromBody(403, { code: 'plan_required' })
    expect(planLimitMessage(error, t, 'pdf export')).toBe(
      'Pdf export is not included in your current plan.',
    )
  })

  it('does not offer an upgrade for a locked feature, even when one is named', () => {
    // A143 gives `feature_locked` exactly one meaning: locked, and no upgrade may
    // be offered to this client. It therefore never names a plan, never claims a
    // limit was reached — the code also covers a device that is over its plan's
    // device count, where the plan is not the problem — and its sentence is
    // different from the `plan_required` one above.
    const error = apiErrorFromBody(403, { code: 'feature_locked', min_plan: 'pro' })
    expect(planLimitMessage(error, t, 'web search')).toBe(
      'Web search is not available on your current plan or device.',
    )
    expect(planLimitMessage(error, t, 'web search')).not.toContain('Pro')
    expect(planLimitMessage(error, t, 'web search')).not.toContain('limit for')
  })

  it('does not describe a plan gate as a reached limit', () => {
    // The two are different facts: `limit_reached` is "you are at the ceiling",
    // `plan_required` is "this was never in your plan". Rendering the second as
    // the first would tell a user to free up something they do not have.
    const gate = apiErrorFromBody(403, { code: 'plan_required', min_plan: 'pro' })
    expect(planLimitMessage(gate, t, 'pdf export')).not.toContain('has reached its limit')
    expect(planLimitMessage(gate, t, 'pdf export')).not.toContain('lifts this limit')
  })

  it('prints a plan id the way the plans config names it', () => {
    expect(planLabel('free')).toBe('Free')
    expect(planLabel('pro')).toBe('Pro')
    expect(planLabel('power')).toBe('Power')
    expect(planLabel('enterprise')).toBe('enterprise')
  })
})

describe('the refusals in Indonesian', () => {
  /**
   * The gap the i18n contract structurally cannot see.
   *
   * `i18n.test.ts` scans the source for `t('…')` calls, and these keys are built
   * by interpolation (`error.${code}.reason`), so a missing entry would pass it
   * and render the key itself to the user. The refusals a student actually hits —
   * at a limit, behind a plan gate, at a locked feature, and out of credits — are
   * asserted in Indonesian, because that is the language the audience reads and
   * the language the copy used to be missing in.
   */
  const inIndonesian = async (run: () => void) => {
    await i18n.changeLanguage('id')
    try {
      run()
    } finally {
      await i18n.changeLanguage('en')
    }
  }

  it('reads the limit refusal in Indonesian', async () => {
    const error = apiErrorFromBody(422, {
      code: 'limit_reached',
      min_plan: 'pro',
      limit: 2,
      used: 2,
    })
    await inIndonesian(() => {
      expect(apiErrorTitle(error, t)).toBe('Batas paket tercapai')
      const sentence = planLimitMessage(error, t, 'ruang belajar')
      expect(sentence).toContain('Paket Pro')
      expect(sentence).toContain('Batasnya 2 dan kamu sudah memakai 2.')
      expect(sentence).toContain('Pro menaikkan batas ini.')
      // Not one English word survives, which is the defect this test exists for.
      expect(sentence).not.toContain('plan has reached')
    })
  })

  it('reads the plan gate, the locked feature and the credit refusal in Indonesian', async () => {
    await inIndonesian(() => {
      const gate = apiErrorFromBody(403, { code: 'plan_required', min_plan: 'pro' })
      expect(apiErrorTitle(gate, t)).toBe('Tidak ada di paketmu')
      expect(planLimitMessage(gate, t, 'ekspor PDF')).toBe(
        'Ekspor PDF tidak termasuk dalam paketmu saat ini. Fitur ini ada di Pro.',
      )

      const locked = apiErrorFromBody(403, { code: 'feature_locked' })
      expect(apiErrorTitle(locked, t)).toBe('Tidak tersedia di paket atau perangkat ini')
      expect(planLimitMessage(locked, t, 'ekspor PDF')).toBe(
        'Ekspor PDF tidak tersedia di paket atau perangkatmu saat ini.',
      )

      const credits = apiErrorFromBody(402, { code: 'insufficient_credits' })
      expect(apiErrorTitle(credits, t)).toBe('Kredit tidak cukup')
      expect(apiErrorReason(credits, t)).toBe('Saldo kreditmu kurang dari biaya ini.')
    })
  })

  it('resolves a key the client raises, in both languages', async () => {
    // A condition the server's code set cannot express carries a key instead of a
    // sentence, because a sentence here is English shown to an Indonesian user —
    // the same gap the tables above had. The key is asserted in both languages and
    // against the generic `network` sentence, since using that one for a
    // mid-answer drop would claim the server was never reached when it was.
    const dropped = new ApiError({
      status: 0,
      code: 'network',
      message: '',
      reasonKey: 'error.network.dropped',
    })
    expect(apiErrorReason(dropped, t)).toBe(
      'The connection dropped before the answer finished. Ask again to get a complete one.',
    )
    expect(apiErrorReason(dropped, t)).not.toBe(apiErrorReason(new ApiError({ status: 0, code: 'network', message: '' }), t))

    await inIndonesian(() => {
      expect(apiErrorReason(dropped, t)).toBe(
        'Koneksi terputus sebelum jawabannya selesai. Tanyakan lagi untuk jawaban yang utuh.',
      )
    })
  })

  it('has a title and a reason in both languages for every code', async () => {
    // The exhaustive check, run in Indonesian as well as English: the tables are
    // keys now, so a code whose copy exists in one file only would otherwise
    // render the key itself to whoever reads the other language.
    for (const language of ['en', 'id'] as const) {
      await i18n.changeLanguage(language)
      for (const code of API_ERROR_CODES) {
        const error = new ApiError({ status: 0, code, message: '' })
        for (const copy of [apiErrorTitle(error, t), apiErrorReason(error, t)]) {
          expect(copy, `${language}: ${code}`).not.toContain('error.')
          expect(copy.length, `${language}: ${code}`).toBeGreaterThan(0)
        }
      }
    }
    await i18n.changeLanguage('en')
  })
})

describe('validation attribution', () => {
  it('uses the only candidate when there is one', () => {
    const error = apiErrorFromBody(400, { code: 'validation', message: 'must not be blank' })
    expect(validationMessage(error, t, ['name'])).toEqual({
      field: 'name',
      message: 'must not be blank',
    })
  })

  it('picks the candidate the server named', () => {
    const error = apiErrorFromBody(400, {
      code: 'validation',
      message: 'password must be at least 12 characters',
    })
    expect(validationMessage(error, t, ['email', 'password', 'device']).field).toBe('password')
  })

  it('leaves the message at the form level when no field is identified', () => {
    const error = apiErrorFromBody(400, { code: 'validation', message: 'request rejected' })
    expect(validationMessage(error, t, ['email', 'password']).field).toBeNull()
  })

  it('falls back to a sentence of its own when the server sent none', () => {
    const error = apiErrorFromBody(400, { code: 'validation', message: '' })
    const { message } = validationMessage(error, t, ['email'])
    expect(message).toBe(apiErrorReason(error, t))
    expect(message.length).toBeGreaterThan(0)
  })
})
