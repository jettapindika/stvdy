import { useMutation, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'
import { platform, type PlatformKind } from '../../platform'
import { clearSession, readRefreshToken, saveSession, setAccessToken } from '../../lib/tokens'

/**
 * Auth: the four calls a session is made of, plus the device identity they carry.
 *
 * A session is not just a token pair. The contract makes the device part of the
 * credential (`device: {name, platform, app_version?}` on signup and login), so
 * the same account on two machines is two sessions that can be revoked
 * independently. That is why the device name is collected at signup rather than
 * left to the server to guess, and why it is defaulted from `platform.kind()`
 * instead of being a required empty field.
 *
 * Establishing a session means three things at once, and they are done in one
 * place so no call site can do two of them: the access token goes into memory,
 * the rotating refresh token and device id go into the secure store, and the
 * `['auth','me']` cache entry is filled so the app can render the signed-in shell
 * without a second round trip.
 */

export interface User {
  id: string
  email: string
  email_verified: boolean
  email_verified_at: string | null
  plan_id: string
  timezone: string
  student_verified: boolean
  /** Display only: every admin route re-reads this from the row, server-side. */
  is_admin: boolean
  created_at: string
}

export interface AuthResponse {
  user: User
  access_token: string
  refresh_token: string
  expires_at: string
  device_id: string
}

export const AUTH_ME_KEY = ['auth', 'me'] as const

/** What a device calls itself in the device list before the user renames it. */
const DEVICE_NAME_BY_KIND: Record<PlatformKind, string> = {
  web: 'Web browser',
  macos: 'Mac',
  windows: 'Windows PC',
  ios: 'iPhone or iPad',
  android: 'Android device',
}

export function defaultDeviceName(kind: PlatformKind = platform.kind()): string {
  return DEVICE_NAME_BY_KIND[kind]
}

/**
 * The device half of a signup or login body.
 *
 * `app_version` is omitted rather than filled with a placeholder: the build does
 * not currently inject a version into the web bundle, and a made-up version
 * string in a device record is worse than an absent one (R-17).
 */
export function currentDevice(name: string = defaultDeviceName()) {
  return { name, platform: platform.kind() }
}

async function establishSession(response: AuthResponse): Promise<AuthResponse> {
  setAccessToken(response.access_token)
  await saveSession({ refreshToken: response.refresh_token, deviceId: response.device_id })
  return response
}

export function useSignUp() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (input: { email: string; password: string; deviceName: string }) => {
      const response = await api.post<AuthResponse>(
        '/auth/signup',
        {
          email: input.email,
          password: input.password,
          device: currentDevice(input.deviceName),
        },
        { auth: false },
      )
      return establishSession(response)
    },
    onSuccess: (response) => {
      queryClient.setQueryData(AUTH_ME_KEY, response.user)
    },
  })
}

export function useSignIn() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async (input: { email: string; password: string }) => {
      const response = await api.post<AuthResponse>(
        '/auth/login',
        {
          email: input.email,
          password: input.password,
          // A returning user is not asked to name the machine again; the device
          // list is where it gets renamed.
          device: currentDevice(),
        },
        { auth: false },
      )
      return establishSession(response)
    },
    onSuccess: (response) => {
      queryClient.setQueryData(AUTH_ME_KEY, response.user)
    },
  })
}

/**
 * Completes a Google sign-in from a single-use ticket.
 *
 * The server's OAuth callback redirects with `?ticket=...` and never with
 * tokens, because a deep link lands in browser history and the OS URL handler
 * and a credential placed there could not be revoked. This consumes the ticket
 * and establishes the session the same way the password path does: the access
 * token goes to memory, the rotating refresh token and device id go to the
 * secure store. The device is created here, not in the callback, so a browser
 * round trip cannot spend a device slot on its own.
 */
export async function completeGoogleSignIn(ticket: string): Promise<AuthResponse> {
  const response = await api.post<AuthResponse>(
    '/auth/google/exchange',
    { ticket, device: currentDevice() },
    { auth: false },
  )
  return establishSession(response)
}

export function useGoogleSignIn() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: completeGoogleSignIn,
    onSuccess: (response) => {
      queryClient.setQueryData(AUTH_ME_KEY, response.user)
    },
  })
}

export function useVerifyEmail() {
  return useMutation({
    mutationFn: (token: string) =>
      api.post<{ verified: boolean }>('/auth/verify-email', { token }, { auth: false }),
  })
}

export function useResendVerification() {
  return useMutation({
    mutationFn: () => api.post<void>('/auth/resend-verification'),
  })
}

/**
 * Ends this session.
 *
 * `POST /auth/logout` revokes the refresh token server-side, so the sign-out is not merely
 * local: a copy of the token that leaked from this device stops working.
 *
 * Native clients send the refresh token in the body (the credential being revoked).
 * Web clients send no body — the server reads the refresh token from the HttpOnly cookie
 * and clears it with `Max-Age=0`. The call is always made on web even when the local
 * store is empty, because the cookie is the credential and the store is a no-op there.
 *
 * The store is cleared even when the request fails. A sign-out that left a working refresh
 * token on the device because the network was down would be a lie about the one action whose
 * whole purpose is to leave, and the failure mode of the alternative is worse than the
 * failure mode of a live token the server will expire anyway.
 */
export function useSignOut() {
  const queryClient = useQueryClient()
  return useMutation({
    mutationFn: async () => {
      const refreshToken = await readRefreshToken()
      if (refreshToken !== null) {
        // Native path: send the token in the body.
        await api.post<void>('/auth/logout', { refresh_token: refreshToken }, { auth: false })
      } else if (platform.runtime() === 'web') {
        // Web path: the cookie carries the credential; server clears it.
        await api.post<void>('/auth/logout', {}, { auth: false })
      }
    },
    onSettled: async () => {
      await clearSession()
      queryClient.clear()
    },
  })
}
