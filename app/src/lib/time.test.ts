import { describe, expect, it } from 'vitest'
import { formatInZone, localDateKey, zoneOffsetLabel } from './time'

describe('time helpers', () => {
  const jakarta = 'Asia/Jakarta'

  it('renders an instant in the user zone, not the machine zone', () => {
    // 2026-10-11T00:00:00Z is 07:00 in Jakarta (UTC+7, no DST).
    const instant = new Date('2026-10-11T00:00:00.000Z')
    expect(
      formatInZone(instant, jakarta, {
        hour: '2-digit',
        minute: '2-digit',
        hour12: false,
      }),
    ).toBe('07:00')
  })

  it('keys a day by the local date, so a late-night session stays on its day', () => {
    // 2026-10-11T17:30:00Z is 2026-10-12T00:30 in Jakarta.
    expect(localDateKey(new Date('2026-10-11T17:30:00.000Z'), jakarta)).toBe('2026-10-12')
    expect(localDateKey(new Date('2026-10-11T16:30:00.000Z'), jakarta)).toBe('2026-10-11')
  })

  it('labels the offset for the given instant', () => {
    expect(zoneOffsetLabel(new Date('2026-10-11T00:00:00.000Z'), jakarta)).toBe('UTC+7')
  })

  it('tracks daylight saving, so a DST zone is not a fixed offset', () => {
    // Europe/London is UTC+1 in July and UTC+0 in January.
    const london = 'Europe/London'
    expect(zoneOffsetLabel(new Date('2026-07-01T12:00:00.000Z'), london)).toBe('UTC+1')
    expect(zoneOffsetLabel(new Date('2026-01-01T12:00:00.000Z'), london)).toBe('UTC+0')
  })

  it('does not shift the date across the UTC boundary', () => {
    // 23:30 UTC is already the next day in Jakarta; a naive UTC key would be wrong.
    expect(localDateKey(new Date('2026-10-11T23:30:00.000Z'), jakarta)).toBe('2026-10-12')
    // ...and the reverse: 17:00 UTC is still the same day in a negative-offset zone.
    expect(localDateKey(new Date('2026-10-11T17:00:00.000Z'), 'America/New_York')).toBe('2026-10-11')
  })
})
