import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api, apiRaw } from '../client'

/**
 * Reports: the outline, its sections, and the Markdown they accumulate into.
 *
 * The two steps are separate calls on purpose, and the shape of this module
 * follows from that. The outline is proposed synchronously and the student edits
 * it before anything else happens; a section is then generated **one at a time**,
 * by index, so fixing section 3 does not pay for sections 1, 2 and 4 again.
 *
 * Three conventions are inherited from `quizzes.ts` rather than reinvented:
 *
 * - the queries poll while something is in flight, the same `refetchInterval`
 *   shape `useDeck` and `useQuiz` use, because a generation job is the same
 *   problem;
 * - the body is never assembled on the client: `body_md` and each section's
 *   `body_md` are what the server stored, so a device holding a partial copy
 *   cannot show a report the server does not have;
 * - the export is a request for a document, not JSON, so it goes through
 *   `apiRaw` — the one place that attaches the bearer token and refreshes a
 *   session once, which a hand-rolled `fetch` would bypass.
 */

/**
 * `draft` before an outline exists, `generating` while a section is being
 * written, `ready` once every section the outline names has one, `failed` when
 * generation gave up.
 */
export type ReportStatus = 'draft' | 'generating' | 'ready' | 'failed'

/**
 * One section of the outline.
 *
 * `gaps` is the contract's honesty field, not decoration: it says this section's
 * topic is not supported by the student's sources, and the screen shows "Not
 * covered in your sources" for it. `labels` are the chunk labels the prompt
 * quoted, and the server re-validates them against the sources on generation.
 */
export interface OutlineSection {
  heading: string
  purpose: string
  labels: number[]
  gaps: boolean
}

/** The outline, as `POST /workspaces/:id/reports/outline` and `PATCH /reports/:id` take it. */
export interface ReportOutline {
  title: string
  sections: OutlineSection[]
}

/**
 * One section's generated text, in outline order.
 *
 * `body_md` is null until the section has been written, which is a different
 * state from written-and-empty: the screen offers "Generate" for the first and
 * "Regenerate" for the second. `generating` is the server's, so the progress
 * survives a reload — the client's own pending mutation is not enough, because
 * the job outlives the tab that started it.
 */
export interface ReportSection {
  heading: string
  body_md: string | null
  generating: boolean
  error: string | null
}

export interface Report {
  id: string
  workspace_id: string
  title: string
  status: ReportStatus
  error: string | null
  /**
   * The editable outline, or null before one has been proposed. The screen
   * cannot rebuild it from `sections`: the outline is the *intent* (a heading, a
   * purpose, the labels that will support it, and whether it is a gap), while a
   * section is the text that came out of it.
   */
  outline: ReportOutline | null
  /** The whole report as stored Markdown, accumulated from the sections. */
  body_md: string | null
  /** One entry per outline section, in outline order. */
  sections: ReportSection[]
  created_at: string
  updated_at: string
}

export interface ReportsResponse {
  reports: Report[]
}

export interface ReportResponse {
  report: Report
}

/**
 * The outline as a screen can render it.
 *
 * `GET /reports/:id` and `GET /workspaces/:id/reports` pass the stored `outline`
 * through **verbatim** — the server's `ReportJson.outline` is the raw `jsonb`
 * column, deliberately, so that what the student edited comes back un-re-encoded
 * and no field can be lost on the way out — and the server derives the top-level
 * `sections` from the stored outline's headings, answering `[]` when there are
 * none. So a row can arrive with a shape `parse_outline` would never have
 * accepted: a hand-written or legacy row with no `sections` array at all, or a
 * section with no `labels`.
 *
 * Both readers of `outline.sections` — the report screen and the Studio's report
 * row — threw `Cannot read properties of undefined (reading 'length')` on such a
 * row and took the screen down with them. The missing pieces are filled in as
 * empty here, in one place, so the two readers cannot disagree about what an
 * outline is; an empty `sections` is what the report screen uses to say out loud
 * that the outline cannot be read, rather than rendering an empty document as
 * though the model had produced nothing.
 */
export function readableOutline(outline: ReportOutline): ReportOutline {
  const sections = Array.isArray(outline.sections) ? outline.sections : []
  return {
    ...outline,
    sections: sections.map((section) => ({
      heading: typeof section.heading === 'string' ? section.heading : '',
      purpose: typeof section.purpose === 'string' ? section.purpose : '',
      labels: Array.isArray(section.labels) ? section.labels : [],
      gaps: section.gaps === true,
    })),
  }
}

export const reportKeys = {
  reports: (workspaceId: string) => ['workspaces', workspaceId, 'reports'] as const,
  report: (reportId: string) => ['reports', reportId] as const,
}

/**
 * The workspace's reports.
 *
 * `GET /workspaces/:id/reports` is not in `docs/API.md` yet, which lists only the
 * outline, the patch, the section generation and the export. The panel has to
 * list something, and the name is the one the sibling lists already use
 * (`/workspaces/:id/decks`, `/workspaces/:id/quizzes`), so this is the
 * convention's name rather than a second one.
 */
export function useReports(workspaceId: string) {
  return useQuery({
    queryKey: reportKeys.reports(workspaceId),
    queryFn: async ({ signal }) =>
      api.get<ReportsResponse>(`/workspaces/${workspaceId}/reports`, { signal }),
    enabled: workspaceId.length > 0,
    // A section being written is the only reason to poll, and the interval is off
    // once every report has settled, so a parked tab stops asking. Both flags are
    // checked: the report's own `generating` while its job is claimed, and a
    // section's `generating` while that one section is in flight — a report whose
    // status has not moved yet must still be asked about again.
    refetchInterval: (query) =>
      (query.state.data?.reports ?? []).some(
        (report) => report.status === 'generating' || report.sections.some((s) => s.generating),
      )
        ? 1500
        : false,
  })
}

export function useReport(reportId: string) {
  return useQuery({
    queryKey: reportKeys.report(reportId),
    queryFn: async ({ signal }) => api.get<ReportResponse>(`/reports/${reportId}`, { signal }),
    enabled: reportId.length > 0,
    // Same condition as the list, for the same reason: the per-section flag is what
    // the screen shows progress from, and it is the flag that must be followed.
    refetchInterval: (query) => {
      const report = query.state.data?.report
      if (report === undefined) return false
      return report.status === 'generating' || report.sections.some((s) => s.generating)
        ? 1500
        : false
    },
  })
}

export interface OutlineInput {
  topic: string
  /** The audience and register the prompt is given, e.g. "exam revision". */
  style: string
  /** The length target the prompt is given, e.g. "short". */
  length: string
  sourceIds: string[]
}

/**
 * Proposes an outline. This is the 3-credit step, and it is deliberately not a
 * job: it is one short completion whose result the student edits before anything
 * else happens, which is the opposite of the long cancellable work the job queue
 * exists for.
 */
export function useCreateOutline(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (input: OutlineInput) =>
      api.post<ReportResponse>(`/workspaces/${workspaceId}/reports/outline`, {
        topic: input.topic,
        style: input.style,
        length: input.length,
        source_ids: input.sourceIds,
      }),
    onSuccess: (response) => {
      // The list is invalidated rather than the new report seeded into the cache.
      // A create response is the server's answer to a create; what the report
      // screen renders is `GET /reports/:id`, and seeding the one from the other
      // would put a shape on screen that the documented route had not confirmed.
      void client.invalidateQueries({ queryKey: reportKeys.reports(workspaceId) })
      void client.invalidateQueries({ queryKey: reportKeys.report(response.report.id) })
    },
  })
}

/**
 * Replaces the outline.
 *
 * The whole outline is sent rather than a patch per field: it is editable, so an
 * edit the generator could not have produced is still valid input as long as it
 * parses, and a field-level patch would need a second shape to describe a
 * rename, a reorder and a delete.
 */
export function useUpdateOutline(reportId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (outline: ReportOutline) =>
      api.patch<ReportResponse>(`/reports/${reportId}`, { outline }),
    onSuccess: () => client.invalidateQueries({ queryKey: reportKeys.report(reportId) }),
  })
}

/**
 * Generates one section by index.
 *
 * The index is the whole point of the call: it is what makes regenerating a
 * single section cheap, and the server replaces that section's text and nothing
 * else. An index outside the outline is the server's to refuse.
 *
 * Every mutation here invalidates rather than seeding the cache from its own
 * response. One route defines what the screen renders — `GET /reports/:id` — so a
 * create, a patch and a generate cannot each put a slightly different report on
 * screen. The refetch is also what carries the section's `generating` flag, which
 * is the progress the student is shown.
 */
export function useGenerateSection(reportId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (index: number) =>
      api.post<ReportResponse>(`/reports/${reportId}/generate`, { index }),
    onSuccess: () => client.invalidateQueries({ queryKey: reportKeys.report(reportId) }),
  })
}

/** The formats a report can be exported as, in the order the menu offers them. */
export const EXPORT_FORMATS = ['md', 'pdf'] as const

export type ExportFormat = (typeof EXPORT_FORMATS)[number]

/**
 * Fetches the exported document.
 *
 * A request through `apiRaw` rather than a plain `<a href>`: the route needs the
 * bearer token, and a navigation cannot carry a header. The bytes come back to
 * the caller, which hands them to the platform's download machinery — this module
 * has no business knowing what a download is. It is a mutation rather than a
 * query because it is an action with a pending state the button has to show.
 */
export function useExportReport(reportId: string) {
  return useMutation({
    mutationFn: async (format: ExportFormat) => {
      const response = await apiRaw(
        `/reports/${reportId}/export?format=${encodeURIComponent(format)}`,
      )
      return { format, blob: await response.blob() }
    },
  })
}
