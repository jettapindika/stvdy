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
}

export const workspaceKeys = {
  all: ['workspaces'] as const,
  detail: (workspaceId: string) => ['workspaces', workspaceId] as const,
}

export function useWorkspaces() {
  return useQuery({
    queryKey: workspaceKeys.all,
    queryFn: async ({ signal }) => {
      const body = await api.get<{ workspaces: Workspace[] }>('/workspaces', { signal })
      return body.workspaces
    },
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

export function useCreateWorkspace() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (name: string) => api.post<{ workspace: Workspace }>('/workspaces', { name }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: workspaceKeys.all }),
  })
}

export function useRenameWorkspace() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: { workspaceId: string; name: string }) =>
      api.patch<{ workspace: Workspace }>(`/workspaces/${input.workspaceId}`, { name: input.name }),
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
