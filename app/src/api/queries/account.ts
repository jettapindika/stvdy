import { useMutation, useQueryClient } from '@tanstack/react-query'

import { api, apiRaw } from '../client'
import { clearSession } from '@/lib/tokens'

/**
 * Account deletion and the data export (A355).
 *
 * Two controls the app store rules require and the spec pins: a student must be able to take
 * their data out and to leave. Neither is a plan feature, so neither is gated — the
 * entitlements matrix's `ExportFormat` governs the *report* and *cheatsheet* documents, not
 * the right to your own data.
 *
 * **The deletion is a request, not a hard delete.** The server marks the account, revokes
 * every session immediately and schedules the purge at the end of the window; the response
 * carries both dates, and this module models them rather than inventing copy about them. What
 * the screen shows is what the server said, which is the only way the two can agree.
 *
 * **Deleting signs this device out, because the server already did.** The request revokes
 * every session family including this one, so the very next request from here is a 401. The
 * local token store is cleared on success so the app moves to `signed-out` deliberately
 * rather than by discovering it through a failed request.
 */

/** The two formats the spec names beside JSON, plus the JSON document itself. */
export const EXPORT_FORMATS = ['json', 'md', 'ics'] as const

export type ExportFormat = (typeof EXPORT_FORMATS)[number]

/** What `DELETE /account` answers. */
export interface DeletionResult {
  /** When the request was recorded. */
  deleted_at: string
  /** When `purge_deleted_accounts` removes the data. */
  purges_at: string
  /**
   * The end of the paid period the account was inside, if any. Forfeited, and stated by the
   * confirmation copy before the student confirms (A350).
   */
  forfeited_period_end: string | null
  sessions_revoked: number
  jobs_cancelled: number
  /** True when the account was already marked, which makes a retried request a no-op. */
  already_deleted: boolean
}

export const accountKeys = {
  all: ['account'] as const,
}

/**
 * Deletes the account.
 *
 * The mutation is not retried: a retry would be a second request whose session the first
 * already revoked, so it could only produce a 401 the student would read as "the deletion
 * failed". The server's own idempotence is what covers the case that matters — two taps
 * racing each other before either commits.
 */
export function useDeleteAccount() {
  const queryClient = useQueryClient()

  return useMutation({
    mutationFn: () => api.delete<DeletionResult>('/account'),
    onSuccess: async () => {
      // The session is already dead server-side. Clearing it here is what moves the app to
      // `signed-out` on purpose instead of through a 401 on the next request.
      await clearSession()
      queryClient.clear()
    },
  })
}

/**
 * Fetches the exported document.
 *
 * A request through `apiRaw` rather than a plain `<a href>`, for the reason the report export
 * is: the route needs the bearer token and a navigation cannot carry a header. The bytes come
 * back to the caller, which hands them to the platform's download machinery — this module has
 * no business knowing what a download is. It is a mutation rather than a query because it is
 * an action with a pending state the button has to show.
 */
export function useExportAccount() {
  return useMutation({
    mutationFn: async (format: ExportFormat) => {
      const response = await apiRaw(`/account/export?format=${encodeURIComponent(format)}`)
      return { format, blob: await response.blob() }
    },
  })
}
