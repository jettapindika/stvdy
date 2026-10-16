import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api, apiRaw } from '../client'

/**
 * Cheatsheets: the JSON blocks the model wrote, the HTML the renderer made of
 * them, and the PDF Chromium made of that.
 *
 * The pipeline is why this module is shaped the way it is. The blocks are the
 * document; the HTML is a pure function of them plus a template version; the PDF
 * is a print of the HTML. So re-rendering an existing cheatsheet costs nothing —
 * no model call, no credits — and the screen can offer it freely, which is what
 * `POST /cheatsheets/:id/re-render` is for: a template fix reaching documents a
 * student already has.
 *
 * `page_count` is read back from the renderer and rendered as it arrives. It is
 * the one number this phase adds, and it is not guessed: the footer's
 * `Halaman N dari M` depends on it, so a number the client computed would be a
 * different number from the one printed on the page.
 */

/**
 * `pending` before the job starts, `generating` while the model is writing the
 * blocks, `ready` once they are stored. A cheatsheet whose PDF failed is still
 * `ready`: the document exists, only its print does not, and that is a retryable
 * render rather than a failed generation.
 */
export type CheatsheetStatus = 'pending' | 'generating' | 'ready' | 'failed'

/** How much material the cheatsheet covers, as `prompts/cheatsheet.v1.md` takes it. */
export type CheatsheetDepth = 'brief' | 'normal' | 'thorough'

export interface Cheatsheet {
  id: string
  workspace_id: string
  title: string
  /** The exam or topic the sheet is for, e.g. "UTS Jaringan Komputer". */
  scope: string
  /** The date the student is revising for, or null when there is none. */
  exam_date: string | null
  depth: CheatsheetDepth
  source_ids: string[]
  status: CheatsheetStatus
  error: string | null
  /** Which `templates/cheatsheet.*.html` rendered it, so an old one is visible. */
  template_version: string | null
  /**
   * The pages the renderer reported for the print. Null until the PDF exists —
   * and a PDF is plan-gated, so on the free plan it stays null rather than being
   * estimated.
   */
  page_count: number | null
  created_at: string
  updated_at: string
}

export interface CheatsheetResponse {
  cheatsheet: Cheatsheet
}

export const cheatsheetKeys = {
  cheatsheets: (workspaceId: string) => ['workspaces', workspaceId, 'cheatsheets'] as const,
  cheatsheet: (cheatsheetId: string) => ['cheatsheets', cheatsheetId] as const,
}

/**
 * The workspace's cheatsheets.
 *
 * Polls while any sheet is still `pending` or `generating`, the same shape the
 * single-sheet query uses: a fresh sheet is `pending` before the job starts.
 */
export function useCheatsheets(workspaceId: string) {
  return useQuery({
    queryKey: cheatsheetKeys.cheatsheets(workspaceId),
    queryFn: async ({ signal }) =>
      api.get<{ cheatsheets: Cheatsheet[] }>(`/workspaces/${workspaceId}/cheatsheets`, {
        signal,
      }),
    enabled: workspaceId.length > 0,
    refetchInterval: (query) =>
      (query.state.data?.cheatsheets ?? []).some(
        (sheet) => sheet.status !== 'ready' && sheet.status !== 'failed',
      )
        ? 1500
        : false,
  })
}

export function useCheatsheet(cheatsheetId: string) {
  return useQuery({
    queryKey: cheatsheetKeys.cheatsheet(cheatsheetId),
    queryFn: async ({ signal }) =>
      api.get<CheatsheetResponse>(`/cheatsheets/${cheatsheetId}`, { signal }),
    enabled: cheatsheetId.length > 0,
    // Anything short of a settled document is worth asking about again: a
    // cheatsheet starts `pending`, moves to `generating`, and only then reaches
    // `ready` or `failed`. Polling on `generating` alone would leave a fresh sheet
    // showing "still being generated" for as long as the tab stayed open, because
    // `pending` is exactly the status it has when the screen mounts.
    refetchInterval: (query) => {
      const status = query.state.data?.cheatsheet.status
      return status === undefined || status === 'ready' || status === 'failed' ? false : 1500
    },
  })
}

export interface CreateCheatsheetInput {
  scope: string
  /** `YYYY-MM-DD`, or null when the student has no date for it. */
  examDate: string | null
  depth: CheatsheetDepth
  sourceIds: string[]
}

export function useCreateCheatsheet(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (input: CreateCheatsheetInput) =>
      api.post<CheatsheetResponse>(`/workspaces/${workspaceId}/cheatsheets`, {
        scope: input.scope,
        exam_date: input.examDate,
        depth: input.depth,
        source_ids: input.sourceIds,
      }),
    onSuccess: (response) => {
      // The new sheet's own query is invalidated, so the screen the panel
      // navigates to renders `GET /cheatsheets/:id` rather than the create
      // response: one route defines what a screen shows.
      void client.invalidateQueries({
        queryKey: cheatsheetKeys.cheatsheet(response.cheatsheet.id),
      })
    },
  })
}

/**
 * Re-renders from the stored blocks.
 *
 * No model call, so it costs nothing and is safe to offer as a plain action. It
 * is the endpoint that makes a template fix reach a document the student may
 * already have printed.
 */
export function useRerenderCheatsheet(cheatsheetId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async () => api.post<CheatsheetResponse>(`/cheatsheets/${cheatsheetId}/re-render`),
    onSuccess: () =>
      client.invalidateQueries({ queryKey: cheatsheetKeys.cheatsheet(cheatsheetId) }),
  })
}

/**
 * The rendered HTML document.
 *
 * `GET /cheatsheets/:id/html` answers the document itself (`text/html`), not a
 * JSON envelope: the plan says the free plan prints the HTML the student already
 * has, which only works if that route serves a document a browser can open. It is
 * fetched here rather than put in an `<iframe src>` because the route needs the
 * bearer token and a frame's navigation cannot carry a header.
 *
 * The document is untrusted input — the model wrote its text — so the frame that
 * renders it is `sandbox`ed. See `CheatsheetPreview`.
 */
export function useCheatsheetHtml(cheatsheetId: string, enabled: boolean) {
  return useQuery({
    queryKey: [...cheatsheetKeys.cheatsheet(cheatsheetId), 'html'] as const,
    queryFn: async () => {
      const response = await apiRaw(`/cheatsheets/${cheatsheetId}/html`)
      return await response.text()
    },
    enabled: cheatsheetId.length > 0 && enabled,
  })
}

/**
 * The PDF.
 *
 * Plan-gated by `export_formats` on the server, which answers 403 `plan_required`
 * when the plan does not include `pdf`. The screen does not rely on that refusal
 * for the UI: it renders no download action at all when the plan does not allow
 * the format, because a phone build must not be told about a purchase.
 */
export function useExportCheatsheetPdf(cheatsheetId: string) {
  return useMutation({
    mutationFn: async () => {
      const response = await apiRaw(`/cheatsheets/${cheatsheetId}/pdf`)
      return await response.blob()
    },
  })
}

/**
 * `DELETE /cheatsheets/:id` — soft-delete a cheatsheet.
 *
 * Invalidates the workspace's cheatsheet list and the dashboard.
 */
export function useDeleteCheatsheet(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (cheatsheetId: string) => api.delete(`/cheatsheets/${cheatsheetId}`),
    onSuccess: async () => {
      await client.invalidateQueries({ queryKey: cheatsheetKeys.cheatsheets(workspaceId) })
      await client.invalidateQueries({ queryKey: ['dashboard'] })
    },
  })
}

/**
 * `PATCH /cheatsheets/:id` — rename a cheatsheet.
 */
export function useRenameCheatsheet(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async ({ cheatsheetId, title }: { cheatsheetId: string; title: string }) =>
      api.patch<CheatsheetResponse>(`/cheatsheets/${cheatsheetId}`, { title }),
    onSuccess: async () => {
      await client.invalidateQueries({ queryKey: cheatsheetKeys.cheatsheets(workspaceId) })
    },
  })
}
