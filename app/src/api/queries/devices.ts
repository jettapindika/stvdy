import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'

/**
 * Devices: the machines this account is signed in on.
 *
 * The route is the contract (`crates/server/src/devices.rs`), and this module is
 * a transcription of it. Two fields carry the honesty rules the Devices screen
 * lives by:
 *
 * - **`last_sync_at`** is when this device last pulled or pushed, written by the
 *   server (`sync::touch_last_sync`, A180). It is deliberately not `last_seen_at`,
 *   which moves on *any* authenticated request: a device that signed up and never
 *   pulled would otherwise read as "synced just now", a number that does not trace
 *   to the work it names. Both are on the wire; the screen shows the one that means
 *   what it says.
 * - **`current`** is the server saying which row is the device making the request.
 *   The screen must not guess from a name the user may have renamed.
 *
 * Revoking is a DELETE and the server answers 204; the list is invalidated rather
 * than patched, because a revoked device's sessions are killed in the same
 * transaction and the list the server would return is the truth.
 */

/** The platforms `devices.platform` holds. */
export type DevicePlatform = 'web' | 'macos' | 'windows' | 'ios' | 'android'

export interface Device {
  id: string
  name: string
  /** Free-form on the wire; the screen maps the five it knows and shows others as-is. */
  platform: string
  app_version: string | null
  last_seen_at: string | null
  /** When this device last pulled or pushed; null until its first pass (A180). */
  last_sync_at: string | null
  created_at: string | null
  revoked_at: string | null
  /** True for the device making the request. */
  current: boolean
}

export const deviceKeys = {
  all: ['devices'] as const,
}

export function useDevices() {
  return useQuery({
    queryKey: deviceKeys.all,
    queryFn: async ({ signal }) => {
      const body = await api.get<{ devices: Device[] }>('/devices', { signal })
      return body.devices
    },
  })
}

export function useRenameDevice() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (input: { deviceId: string; name: string }) =>
      api.patch<{ device: Device }>(`/devices/${input.deviceId}`, { name: input.name }),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: deviceKeys.all }),
  })
}

/**
 * Signs a device out remotely.
 *
 * The server revokes the device and every session it holds in one transaction, so
 * the next request from that machine is 401 and its local cache is wiped on
 * contact (A186). Nothing is sent in the body: the device id in the path is the
 * whole instruction.
 */
export function useRevokeDevice() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: (deviceId: string) => api.delete(`/devices/${deviceId}`),
    onSuccess: () => queryClient.invalidateQueries({ queryKey: deviceKeys.all }),
  })
}
