/**
 * Decorative background illustration for course cards.
 *
 * Renders one of six abstract geometric patterns per course colour index (0–5),
 * inheriting its hue from the card's `--course-marker` custom property set by
 * `courseVars()`. The pattern sits at low opacity so it reads as a subtle
 * texture behind the card content, not a foreground graphic.
 *
 * **All patterns are purely abstract.** Earlier versions drew subject-flavoured
 * glyphs (equations, hearts, stars) keyed to an arbitrary round-robin index,
 * which meant a law course could render calculus fragments. Ruled lines, grids,
 * dots and hatching carry no subject claim.
 */

/** Horizontal ruled lines, like a notebook page. */
function RuledLines() {
  const ys = [20, 40, 60, 80, 100, 120, 140]
  return (
    <g opacity="0.12" stroke="var(--course-marker, var(--course-indigo))" strokeWidth="1">
      {ys.map((y) => (
        <line key={y} x1="16" y1={y} x2="384" y2={y} />
      ))}
    </g>
  )
}

/** A grid of evenly spaced dots. */
function DotGrid() {
  const dots: [number, number][] = []
  for (let x = 30; x <= 370; x += 40) {
    for (let y = 20; y <= 140; y += 30) {
      dots.push([x, y])
    }
  }
  return (
    <g opacity="0.14" fill="var(--course-marker, var(--course-indigo))">
      {dots.map(([x, y]) => (
        <circle key={`${x}-${y}`} cx={x} cy={y} r="2.5" />
      ))}
    </g>
  )
}

/** Concentric circles at different positions. */
function Ripples() {
  const centres: [number, number, number[]][] = [
    [120, 80, [30, 50, 70]],
    [300, 60, [25, 45]],
    [200, 130, [20, 40, 60]],
  ]
  return (
    <g
      opacity="0.10"
      stroke="var(--course-marker, var(--course-indigo))"
      fill="none"
      strokeWidth="1.2"
    >
      {centres.map(([cx, cy, radii]) =>
        radii.map((r) => <circle key={`${cx}-${cy}-${r}`} cx={cx} cy={cy} r={r} />),
      )}
    </g>
  )
}

/** Diagonal hatching lines. */
function Hatching() {
  const lines: [number, number, number, number][] = []
  for (let offset = -160; offset <= 560; offset += 24) {
    lines.push([offset, 0, offset + 160, 160])
  }
  return (
    <g opacity="0.10" stroke="var(--course-marker, var(--course-indigo))" strokeWidth="1">
      {lines.map(([x1, y1, x2, y2]) => (
        <line key={x1} x1={x1} y1={y1} x2={x2} y2={y2} />
      ))}
    </g>
  )
}

/** Cross-hatch: two sets of diagonal lines. */
function CrossHatch() {
  const forward: [number, number, number, number][] = []
  const backward: [number, number, number, number][] = []
  for (let offset = -160; offset <= 560; offset += 32) {
    forward.push([offset, 0, offset + 160, 160])
    backward.push([offset + 160, 0, offset, 160])
  }
  return (
    <g opacity="0.08" stroke="var(--course-marker, var(--course-indigo))" strokeWidth="1">
      {forward.map(([x1, y1, x2, y2]) => (
        <line key={`f${x1}`} x1={x1} y1={y1} x2={x2} y2={y2} />
      ))}
      {backward.map(([x1, y1, x2, y2]) => (
        <line key={`b${x1}`} x1={x1} y1={y1} x2={x2} y2={y2} />
      ))}
    </g>
  )
}

/** Horizontal dashed lines of varying lengths. */
function DashedLines() {
  const rows = [
    { y: 20, segments: [[20, 120], [140, 280], [300, 380]] },
    { y: 45, segments: [[40, 180], [200, 360]] },
    { y: 70, segments: [[20, 100], [120, 240], [260, 380]] },
    { y: 95, segments: [[60, 200], [220, 340]] },
    { y: 120, segments: [[20, 160], [180, 300], [320, 380]] },
    { y: 145, segments: [[40, 140], [160, 380]] },
  ]
  return (
    <g
      opacity="0.12"
      stroke="var(--course-marker, var(--course-indigo))"
      strokeWidth="1.5"
      strokeLinecap="round"
    >
      {rows.map(({ y, segments }) =>
        segments.map(([x1, x2]) => <line key={`${y}-${x1}`} x1={x1} y1={y} x2={x2} y2={y} />),
      )}
    </g>
  )
}

const patterns = [RuledLines, DotGrid, Ripples, Hatching, CrossHatch, DashedLines] as const

export function CourseIllustration({
  color,
  className,
}: {
  /** Course colour index, 0–5. Out-of-range falls back to the first pattern. */
  color: number
  className?: string
}) {
  const Pattern = patterns[color >= 0 && color < patterns.length ? color : 0] as (typeof patterns)[0]
  return (
    <svg
      viewBox="0 0 400 160"
      className={className}
      aria-hidden="true"
      preserveAspectRatio="xMidYMid slice"
    >
      <Pattern />
    </svg>
  )
}
