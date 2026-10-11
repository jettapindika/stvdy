/**
 * The outline icon set.
 *
 * One set, one stroke weight, `currentColor` only, so an icon inherits the text
 * colour of whatever it sits in and cannot introduce a colour the contrast gate
 * has not measured. The spec asks for a bundled outline set at 20px (24px in tab
 * bars) and for an icon rather than an emoji wherever a state is marked, which is
 * why the streak and the lock are drawn here rather than typed.
 *
 * Icons are added as a screen needs them. A set of two hundred mostly-unused
 * paths is weight in the bundle and surface for inconsistency, and the allowlist
 * below is what keeps `name` a real type instead of a string.
 */
export type IconName =
  | 'lock'
  | 'streak'
  | 'check'
  | 'clock'
  | 'cards'
  | 'chevron-right'
  | 'external'
  | 'search'
  | 'plus'
  | 'close'
  | 'alert'

const PATHS: Record<IconName, string> = {
  // A closed padlock: the locked-feature mark, and the read-only mark on a
  // Google/ICS event.
  lock: 'M7 10V7a5 5 0 0 1 10 0v3M5 10h14v10H5z',
  // A flame for the streak. Outline, like everything else; the streak is a
  // gentle fact, not a badge.
  streak:
    'M12 3c1.5 3 4 4.5 4 8a4 4 0 0 1-8 0c0-1.2.4-2.2 1-3 .2 1.2.9 2 1.8 2.2C11.5 8.4 12 5.8 12 3z',
  check: 'M4 12.5 9 17.5 20 6.5',
  clock: 'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18zM12 7.5V12l3 2',
  // Two stacked cards, for a deck.
  cards: 'M4 8.5h12v10H4zM7 5.5h12v10',
  'chevron-right': 'M9 5l7 7-7 7',
  external: 'M14 4h6v6M20 4l-9 9M18 14v5H5V6h5',
  search: 'M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16zM17 17l4 4',
  plus: 'M12 5v14M5 12h14',
  close: 'M6 6l12 12M18 6L6 18',
  // A triangle with an exclamation: the "not covered in your sources" caveat on a
  // report outline. A caveat, not a failure, so it is drawn as a warning sign
  // rather than as anything that reads as an error.
  alert: 'M12 4l9 16H3zM12 10v4M12 17h.01',
}

export function Icon({
  name,
  size = 20,
  className,
}: {
  name: IconName
  /** 20px by default, 24px in a tab bar. */
  size?: number
  className?: string
}) {
  return (
    <svg
      className={className}
      width={size}
      height={size}
      viewBox="0 0 24 24"
      fill="none"
      stroke="currentColor"
      strokeWidth={1.75}
      strokeLinecap="round"
      strokeLinejoin="round"
      // Decorative by default. An icon that carries meaning on its own (an icon
      // button with no label) is the caller's job to name, and an icon that
      // repeats an adjacent word should stay out of the accessibility tree.
      aria-hidden="true"
      focusable="false"
    >
      <path d={PATHS[name]} />
    </svg>
  )
}
