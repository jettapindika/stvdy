import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'

/**
 * Workspaces: the container a course's material lives in.
 *
 * `source_count` is on the list response, so the list does not need a second
 * request per row to say how much is in a workspace. Mutations invalidate the
 * list rather than patching it in place: the server is the source of truth for
 * `updated_at` and `source_count`, and a hand-patched cache drifts from it.
 */

export interface Workspace {
  id: string
  name: string
  created_at: string
  updated_at: string
  source_count: number
  /**
   * The course code the student gave this workspace (`BIO 201`), or null.
   *
   * Optional on purpose: the design's sidebar and course cards show a code, but a
   * workspace created from the API alone has none, and inventing one would be a
   * fact the app made up. Every surface that renders it falls back to the name.
   */
  course_code: string | null
  /**
   * The course's hue, 0-5, indexing the six categorical course colours. Assigned
   * round-robin by the server on create so two adjacent courses differ, and
   * never a status colour.
   */
  color: number
  /** The next exam for this course, `YYYY-MM-DD`, or null. */
  exam_date: string | null
}

export const workspaceKeys = {
  all: ['workspaces'] as const,
  detail: (workspaceId: string) => ['workspaces', workspaceId] as const,
}

/**
 * The workspace list.
 *
 * `enabled` exists for the sidebar's course list: the rail is desktop-only, and a
 * phone should not fetch the list a second time just to render a list it does not
 * show. The screen itself calls this with no options.
 */
export function useWorkspaces({ enabled = true }: { enabled?: boolean } = {}) {
  return useQuery({
    queryKey: workspaceKeys.all,
    queryFn: async ({ signal }) => {
      const body = await api.get<{ workspaces: Workspace[] }>('/workspaces', { signal })
      return body.workspaces
    },
    enabled,
  })
}

export function useWorkspace(workspaceId: string) {
  return useQuery({
    queryKey: workspaceKeys.detail(workspaceId),
    queryFn: async ({ signal }) => {
      const body = await api.get<{ workspace: Workspace }>(`/workspaces/${workspaceId}`, { signal })
      return body.workspace
    },
    enabled: workspaceId.length > 0,
  })
}

/**
 * The course fields a create or an edit may set.
 *
 * `course_code` and `exam_date` are sent as `null` when the form's field is empty,
 * never omitted: `PATCH` distinguishes an absent field ("leave it alone") from a
 * `null` one ("clear it"), so a form that omitted an emptied field could never
 * remove a code or a date once it had one.
 */
export interface WorkspaceFields {
  name: string
  courseCode: string
  examDate: string
}

function courseBody(fields: WorkspaceFields) {
  const code = fields.courseCode.trim()
  const exam = fields.examDate.trim()
  return {
    name: fields.name,
    course_code: code.length > 0 ? code : null,
    exam_date: exam.length > 0 ? exam : null,
  }
}

export function useCreateWorkspace() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (fields: WorkspaceFields) =>
      api.post<{ workspace: Workspace }>('/workspaces', courseBody(fields)),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: workspaceKeys.all }),
  })
}

export function useRenameWorkspace() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: WorkspaceFields & { workspaceId: string }) =>
      api.patch<{ workspace: Workspace }>(
        `/workspaces/${input.workspaceId}`,
        courseBody(input),
      ),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: workspaceKeys.all }),
  })
}

export function useDeleteWorkspace() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (workspaceId: string) => api.delete(`/workspaces/${workspaceId}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: workspaceKeys.all }),
  })
}
