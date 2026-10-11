/**
 * Timezone helpers.
 *
 * Every instant is stored and transported as UTC; every *local* computation goes
 * through the user's IANA zone (default Asia/Jakarta). Never use the machine
 * zone: a student in Jakarta on a device set to UTC would otherwise see study
 * blocks on the wrong day.
 */

export const DEFAULT_TIMEZONE = 'Asia/Jakarta'

export function formatInZone(
  instant: Date,
  timeZone: string,
  options: Intl.DateTimeFormatOptions = { dateStyle: 'medium', timeStyle: 'short' },
): string {
  return new Intl.DateTimeFormat('en-GB', { ...options, timeZone }).format(instant)
}

/** The calendar day the instant falls on, in the user's zone (YYYY-MM-DD). */
export function localDateKey(instant: Date, timeZone: string): string {
  // en-CA renders as YYYY-MM-DD.
  return new Intl.DateTimeFormat('en-CA', {
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    timeZone,
  }).format(instant)
}

/** e.g. "UTC+7" for Asia/Jakarta at the given instant (DST-aware). */
export function zoneOffsetLabel(instant: Date, timeZone: string): string {
  const part = new Intl.DateTimeFormat('en-GB', { timeZone, timeZoneName: 'longOffset' })
    .formatToParts(instant)
    .find((p) => p.type === 'timeZoneName')?.value

  // "GMT+07:00" -> "UTC+7"; "GMT" -> "UTC+0".
  if (!part) return 'UTC+0'
  const match = /GMT([+-])(\d{2}):(\d{2})/.exec(part)
  if (!match) return 'UTC+0'
  const [, sign, hours, minutes] = match
  const h = Number(hours)
  return minutes === '00' ? `UTC${sign}${h}` : `UTC${sign}${h}:${minutes}`
}
