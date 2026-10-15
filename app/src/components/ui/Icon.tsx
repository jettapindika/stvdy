/**
 * The outline icon set.
 *
 * One set, one stroke weight, `currentColor` only, so an icon inherits the text
 * colour of whatever it sits in and cannot introduce a colour the contrast gate
 * has not measured. The design asks for 20px in controls and 24px in a tab bar,
 * and for an icon rather than an emoji wherever a state is marked — which is why
 * the streak is drawn here rather than typed as a flame character.
 *
 * **This replaces the earlier "no icons, deliberately" decision.** The shipped
 * sidebar had no glyphs at all, on the grounds that a thin-stroke icon set would
 * give the app the look of every other generated interface. The design's
 * navigation item is an icon plus a label, and the design is now the requirement,
 * so the set exists. What survives from the earlier reasoning is the *discipline*:
 * one bundled family drawn on a 24px grid, never an imported pack, and never an
 * emoji standing in for a state.
 *
 * Icons are added as a screen needs them. A set of two hundred mostly-unused
 * paths is weight in the bundle and surface for inconsistency, and the allowlist
 * below is what keeps `name` a real type instead of a string.
 *
 * Every glyph is stroked, never filled, and drawn to be legible at 20px: a path
 * that needs a 1px detail to read is a path that will look like dirt on a phone.
 */
export type IconName =
  // Navigation
  | 'today'
  | 'workspaces'
  | 'study'
  | 'planner'
  | 'settings'
  // Structure and objects
  | 'calendar'
  | 'cards'
  | 'file-text'
  | 'layers'
  | 'list-checks'
  | 'chart'
  | 'presentation'
  | 'lock'
  // Devices. The two shapes a signed-in device is, so a device row's symbol is a
  // picture of the machine rather than a word repeated beside its own name.
  | 'laptop'
  | 'smartphone'
  // Actions
  | 'search'
  | 'plus'
  | 'close'
  | 'check'
  | 'circle-check'
  | 'copy'
  | 'bookmark'
  | 'undo'
  | 'upload'
  | 'link'
  | 'play'
  | 'external'
  | 'ellipsis'
  // A reorder handle, and the sync pass that can be asked for by hand.
  | 'grip-vertical'
  | 'refresh-cw'
  // Marks and states
  | 'sparkles'
  | 'help'
  | 'globe'
  | 'streak'
  | 'clock'
  | 'alert'
  // Chevrons and arrows
  | 'chevron-left'
  | 'chevron-right'
  | 'chevron-down'
  | 'arrow-left'
  | 'expand'
  // Attachment
  | 'paperclip'

/**
 * Paths per icon. A list rather than a single `d`, because several of these are
 * two or three strokes (the folder is an outline plus a tab, the gear is a ring
 * plus its teeth) and joining them into one `d` would make each one unreadable
 * and impossible to correct.
 */
const PATHS: Record<IconName, string[]> = {
  // --- Navigation ---------------------------------------------------------
  // A dashboard: one tall pane and three small ones.
  today: ['M4 4h7v7H4zM15 4h5v4h-5zM15 12h5v8h-5zM4 15h7v5H4z'],
  // A folder: the workspace.
  workspaces: ['M3 7a2 2 0 0 1 2-2h3.5l2 2H19a2 2 0 0 1 2 2v8a2 2 0 0 1-2 2H5a2 2 0 0 1-2-2z'],
  // An open book: study.
  study: [
    'M12 6.5C10.4 5 8.3 4.4 5 4.4a1 1 0 0 0-1 1v11a1 1 0 0 0 1 1c3.3 0 5.4.6 7 2.1 1.6-1.5 3.7-2.1 7-2.1a1 1 0 0 0 1-1v-11a1 1 0 0 0-1-1c-3.3 0-5.4.6-7 2.1z',
    'M12 6.5v13',
  ],
  planner: [
    'M4 6.5A1.5 1.5 0 0 1 5.5 5h13A1.5 1.5 0 0 1 20 6.5v13A1.5 1.5 0 0 1 18.5 21h-13A1.5 1.5 0 0 1 4 19.5z',
    'M4 10h16M8 3v4M16 3v4',
  ],
  // A gear: the ring, then four teeth as short strokes. A full eight-tooth gear
  // is a shape that turns to mud below 24px.
  settings: [
    'M12 15.5a3.5 3.5 0 1 0 0-7 3.5 3.5 0 0 0 0 7z',
    'M12 2.5v3M12 18.5v3M2.5 12h3M18.5 12h3M5.2 5.2l2.1 2.1M16.7 16.7l2.1 2.1M18.8 5.2l-2.1 2.1M7.3 16.7l-2.1 2.1',
  ],

  // --- Structure and objects ---------------------------------------------
  calendar: [
    'M4 6.5A1.5 1.5 0 0 1 5.5 5h13A1.5 1.5 0 0 1 20 6.5v13A1.5 1.5 0 0 1 18.5 21h-13A1.5 1.5 0 0 1 4 19.5z',
    'M4 10h16M8 3v4M16 3v4',
  ],
  // Two stacked cards, for a deck.
  cards: ['M4 8.5h12v10H4zM7 5.5h12v10'],
  'file-text': [
    'M14 3H7a2 2 0 0 0-2 2v14a2 2 0 0 0 2 2h10a2 2 0 0 0 2-2V8z',
    'M14 3v5h5M9 13h6M9 17h4',
  ],
  // Three offset planes: the study queue, and "Due Now".
  layers: ['M12 3.5l8.5 4.5-8.5 4.5-8.5-4.5z', 'M3.5 12.5l8.5 4.5 8.5-4.5'],
  'list-checks': ['M4 6.5h2.5M4 12h2.5M4 17.5h2.5M10 6.5h10M10 12h10M10 17.5h10'],
  // Three columns of different heights: a bar chart, without axes.
  chart: ['M6 20V10M12 20V4M18 20v-6'],
  presentation: ['M4 3.5h16M5.5 3.5v10.5h13V3.5M12 14v3.5M9 21l3-3.5 3 3.5'],
  // A closed padlock: the locked-feature mark, and the read-only mark on a
  // Google/ICS event.
  lock: ['M7 10.5V7a5 5 0 0 1 10 0v3.5', 'M5 10.5h14v10H5z'],
  // An open laptop: the lid, then the deck it rests on. Drawn from the front
  // rather than in perspective, because a foreshortened lid is a shape that turns
  // to mud at 24px.
  laptop: ['M5 6a1.5 1.5 0 0 1 1.5-1.5h11A1.5 1.5 0 0 1 19 6v9H5z', 'M2.5 18.5h19'],
  // A handset: the body, then the one line that says which way up it is.
  smartphone: [
    'M8 3.5h8A1.5 1.5 0 0 1 17.5 5v14A1.5 1.5 0 0 1 16 20.5H8A1.5 1.5 0 0 1 6.5 19V5A1.5 1.5 0 0 1 8 3.5z',
    'M11 17.2h2',
  ],

  // --- Actions ------------------------------------------------------------
  search: ['M11 19a8 8 0 1 0 0-16 8 8 0 0 0 0 16z', 'M17 17l4 4'],
  plus: ['M12 5v14M5 12h14'],
  close: ['M6 6l12 12M18 6L6 18'],
  check: ['M4 12.5 9 17.5 20 6.5'],
  'circle-check': ['M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z', 'M8.2 12.4l2.6 2.6 5-5.4'],
  copy: ['M9 9h10v10H9z', 'M15 5.5H5.5V15'],
  bookmark: ['M6.5 4h11v17l-5.5-4-5.5 4z'],
  undo: ['M9 14L4 9l5-5', 'M4 9h10a6 6 0 0 1 0 12h-4'],
  upload: ['M12 16V4M7 9l5-5 5 5M4 20h16'],
  link: [
    'M10.5 13.5a4.5 4.5 0 0 0 6.4 0l2.1-2.1a4.5 4.5 0 0 0-6.4-6.4l-1 1',
    'M13.5 10.5a4.5 4.5 0 0 0-6.4 0l-2.1 2.1a4.5 4.5 0 0 0 6.4 6.4l1-1',
  ],
  play: ['M7.5 4.5l12 7.5-12 7.5z'],
  external: ['M14 4h6v6M20 4l-9 9M18 14v5H5V6h5'],
  // Three dots. Drawn as tiny circles rather than strokes with a round cap,
  // because a zero-length stroke is not reliably rendered and the dots are the
  // whole glyph.
  ellipsis: [
    'M6 12a1.1 1.1 0 1 0 2.2 0 1.1 1.1 0 0 0-2.2 0z',
    'M10.9 12a1.1 1.1 0 1 0 2.2 0 1.1 1.1 0 0 0-2.2 0z',
    'M15.8 12a1.1 1.1 0 1 0 2.2 0 1.1 1.1 0 0 0-2.2 0z',
  ],

  // Six dots in two columns. Drawn as tiny circles rather than zero-length
  // strokes, for the reason `ellipsis` gives.
  'grip-vertical': [
    'M8.1 6a0.9 0.9 0 1 0 1.8 0 0.9 0.9 0 0 0-1.8 0z',
    'M14.1 6a0.9 0.9 0 1 0 1.8 0 0.9 0.9 0 0 0-1.8 0z',
    'M8.1 12a0.9 0.9 0 1 0 1.8 0 0.9 0.9 0 0 0-1.8 0z',
    'M14.1 12a0.9 0.9 0 1 0 1.8 0 0.9 0.9 0 0 0-1.8 0z',
    'M8.1 18a0.9 0.9 0 1 0 1.8 0 0.9 0.9 0 0 0-1.8 0z',
    'M14.1 18a0.9 0.9 0 1 0 1.8 0 0.9 0.9 0 0 0-1.8 0z',
  ],
  // Two arcs, each with its own arrowhead: the sync pass. Two rather than one
  // because a single arc reads as a loading spinner, which is a different claim.
  'refresh-cw': [
    'M3 12a9 9 0 0 1 15.5-6.2',
    'M18.5 2v4h-4',
    'M21 12a9 9 0 0 1-15.5 6.2',
    'M5.5 22v-4h4',
  ],

  // --- Marks and states ---------------------------------------------------
  // The assistant mark: a four-point sparkle with a small one beside it. It
  // marks anything the AI produced or can do, and it is never used as
  // decoration on a screen with no AI in it.
  sparkles: [
    'M10.5 3.5l1.8 4.7 4.7 1.8-4.7 1.8-1.8 4.7-1.8-4.7L4 10l4.7-1.8z',
    'M17.5 14.5l.9 2.3 2.3.9-2.3.9-.9 2.3-.9-2.3-2.3-.9 2.3-.9z',
  ],
  help: [
    'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z',
    'M9.4 9.4a2.7 2.7 0 0 1 5.2.9c0 1.8-2.6 2.2-2.6 4M12 17.3h.01',
  ],
  globe: [
    'M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z',
    'M3.2 12h17.6M12 3c2.4 2.6 3.7 5.6 3.7 9s-1.3 6.4-3.7 9c-2.4-2.6-3.7-5.6-3.7-9S9.6 5.6 12 3z',
  ],
  // A flame for the streak. Outline, like everything else; the streak is a
  // gentle fact, not a badge.
  streak: [
    'M12 3c1.5 3 4 4.5 4 8a4 4 0 0 1-8 0c0-1.2.4-2.2 1-3 .2 1.2.9 2 1.8 2.2C11.5 8.4 12 5.8 12 3z',
  ],
  clock: ['M12 21a9 9 0 1 0 0-18 9 9 0 0 0 0 18z', 'M12 7.5V12l3 2'],
  // A triangle with an exclamation: the "not covered in your sources" caveat on
  // a report outline. A caveat, not a failure, so it is drawn as a warning sign
  // rather than as anything that reads as an error.
  alert: ['M12 4l9 16H3z', 'M12 10v4M12 17h.01'],

  // --- Chevrons and arrows ------------------------------------------------
  'chevron-left': ['M15 19l-7-7 7-7'],
  'chevron-right': ['M9 5l7 7-7 7'],
  'chevron-down': ['M6 9.5l6 6 6-6'],
  'arrow-left': ['M19 12H5', 'M11 18l-6-6 6-6'],
  // Four outward arrows: the expand / full-screen control on the PDF viewer.
  expand: [
    'M4 14v6h6M20 10V4h-6M14 10l6-6M10 14l-6 6',
  ],
  // A rotated paperclip: the attach-file control in the composer.
  paperclip: [
    'M21.4 11.6l-8.5 8.5a4.5 4.5 0 0 1-6.4-6.4l8.5-8.5a3 3 0 0 1 4.3 4.3l-8.5 8.5a1.5 1.5 0 0 1-2.1-2.1l8.5-8.5',
  ],
}

export type IconProps = {
  name: IconName
  /** 20px by default, 24px in a tab bar or a mobile app header. */
  size?: number
  className?: string
}

export function Icon({ name, size = 20, className }: IconProps) {
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
      {PATHS[name].map((d) => (
        <path key={d} d={d} />
      ))}
    </svg>
  )
}
