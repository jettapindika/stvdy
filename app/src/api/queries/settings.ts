import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'
import { platform } from '@/platform'
import { hasLocalCache } from './synced'

/**
 * Settings: the synced preferences row, and the account's timezone beside it.
 *
 * Two rules from the server shape this module, and both are visible in its types.
 *
 * **The timezone is not on the row** (A203). It lives on `users`, it predates any
 * settings row, and the reminder and planner maths read it for an account that has
 * never opened this screen. `GET /settings` therefore returns it *beside* the row
 * and `PUT /settings` writes it to `users`, so this module models the response as
 * a pair rather than inventing a `timezone` field on `SettingsRow` that no pull
 * would ever fill.
 *
 * **A write is partial and whole-row.** Partial, because the screen saves per
 * section and a full replace would let a stale tab clobber a channel set changed
 * on another device; whole-row, because `user_settings` gets record-level LWW and
 * there is no per-field timestamp to merge with. So `saveSettings` sends only what
 * the caller changed and the server writes the merged row.
 *
 * A channel the plan does not grant is refused per channel with 403
 * `plan_required`, and the screen's job is to show it locked *before* that — which
 * is why the screen reads `GET /usage`'s `features.reminder_channels` rather than
 * offering the channel and handling the refusal.
 */

export type ReminderChannel = 'web' | 'push' | 'email' | 'telegram' | 'discord'

export interface SettingsRow {
  study_start_hour: number
  study_end_hour: number
  max_study_min_per_day: number
  block_len_min: number
  break_min: number
  reminder_channels: ReminderChannel[]
  /** A pair or neither; the window may wrap midnight (A202). */
  quiet_start_hour: number | null
  quiet_end_hour: number | null
  language: 'en' | 'id'
  theme: 'system' | 'light' | 'dark'
  updated_at: string
}

export interface SettingsView {
  settings: SettingsRow
  /** `users.timezone`, which is why it is not a field of the row above (A203). */
  timezone: string
  /**
   * The reminder channels this deployment can actually deliver on (A288).
   *
   * Not a field of the row, and not in the cache, because it is not the student's
   * preference: it is a fact about which transports exist — `push` is present only
   * when the server has APNs/FCM credentials — and the settings screen offers only
   * what will fire. The server derives it; the client never guesses it.
   *
   * Empty means "this build could not ask", which is the safe direction: a channel
   * offered because the answer was unknown is exactly the channel that is granted,
   * selectable and silently never fires, which is the defect the field exists to
   * remove.
   */
  channels_available: string[]
}

export const settingsKeys = {
  all: ['settings'] as const,
}

/** The fields a save may name. Every one is optional: this is a partial write. */
export type SettingsPatch = Partial<Omit<SettingsRow, 'updated_at'> & { timezone: string }>

/**
 * The deployment's available channels, from the route, or none.
 *
 * A failure is `[]` rather than an error, and that is the whole point: the cached
 * path below reads the *row* from the device, and a settings screen that refused
 * to render because one metadata field could not be fetched would be a settings
 * screen that does not work offline — which is the thing the cache exists for. An
 * empty answer disables every channel row instead, which is honest and visible.
 */
async function readAvailableChannels(): Promise<string[]> {
  try {
    const body = await api.get<{ channels_available: string[] }>('/settings')
    return Array.isArray(body.channels_available) ? body.channels_available : []
  } catch {
    return []
  }
}

export function useSettings() {
  return useQuery({
    queryKey: settingsKeys.all,
    queryFn: async (): Promise<SettingsView> => {
      if (hasLocalCache()) {
        // The settings row is one row, and its primary key *is* the user's id
        // (A200), so the cached read is the same row the pull applies. The
        // timezone is on `users` and therefore not in the cache; the screen reads
        // it from `GET /auth/me`, which it already has.
        const rows = await platform.sync.read<SettingsRow>('user_settings')
        const settings = rows[0]
        if (settings === undefined) throw new Error('the settings row is not cached yet')
        return { settings, timezone: '', channels_available: await readAvailableChannels() }
      }
      return await api.get<SettingsView>('/settings')
    },
  })
}

/**
 * Saves a partial settings change.
 *
 * On the cache path the mutation id is the **user's id**, because
 * `user_settings`' primary key is `user_id` and the table has no `id` column, so
 * the contract's id has nothing else to be (A200). A mutation carrying another id
 * is rejected by the server rather than applied, which is why the caller passes
 * the id it is writing as.
 */
export async function saveSettings(
  userId: string,
  patch: SettingsPatch,
): Promise<void> {
  if (hasLocalCache()) {
    await platform.sync.write('user_settings', userId, { ...patch })
    return
  }
  await api.put<SettingsView>('/settings', { ...patch })
}

export function useSaveSettings(userId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: (patch: SettingsPatch) => saveSettings(userId, patch),
    onSuccess: () => client.invalidateQueries({ queryKey: settingsKeys.all }),
  })
}
