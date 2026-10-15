import type { ReactNode } from 'react'

/**
 * An SVG toggle switch matching the Figma Stvdy / Switch specimens (411:19778,
 * 411:19779). The track is a 24×24 pill; the knob is a 6px-radius circle that
 * sits left (off) or right (on).
 *
 * The ON state fills the track with the brand colour and shows white strokes;
 * the OFF state is an outlined track with `currentColor`. The knob position
 * is the non-colour signal, so the two states differ in monochrome.
 *
 * The native `<input type="checkbox" role="switch">` is visually hidden but
 * remains in the accessibility tree, so screen readers announce "on"/"off" and
 * the label wrapping it is the accessible name. The SVG is `aria-hidden`.
 *
 * Focus-visible styling is on the label via `has-[:focus-visible]`, matching
 * the pattern `SourcePicker.tsx:68` and `SyllabusReviewScreen.tsx:230` use.
 */
export function Toggle({
  checked,
  onChange,
  disabled,
  children,
  className,
  title,
  describedBy,
}: {
  checked: boolean
  onChange: (next: boolean) => void
  disabled?: boolean
  /** The visible label beside the switch. */
  children: ReactNode
  className?: string
  /** Tooltip on the label. */
  title?: string
  /** The id of the element that describes this switch. */
  describedBy?: string
}) {
  return (
    <label
      className={`flex min-h-11 cursor-pointer items-center gap-3 text-label has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-ring rounded-field ${disabled ? 'opacity-45 cursor-not-allowed' : ''} ${className ?? ''}`}
      title={title}
    >
      <input
        type="checkbox"
        role="switch"
        className="sr-only"
        checked={checked}
        disabled={disabled}
        aria-describedby={describedBy}
        onChange={(event) => {
          onChange(event.target.checked)
        }}
      />
      <svg
        width={24}
        height={24}
        viewBox="0 0 24 24"
        fill="none"
        aria-hidden="true"
        className="shrink-0"
      >
        {/* Track: pill shape matching the Figma SVG export paths */}
        <path
          d="M16 5H8C4.13401 5 1 8.13401 1 12C1 15.866 4.13401 19 8 19H16C19.866 19 23 15.866 23 12C23 8.13401 19.866 5 16 5Z"
          fill={checked ? 'var(--primary)' : 'transparent'}
          stroke={checked ? 'var(--primary)' : 'currentColor'}
          strokeWidth={2}
          strokeLinecap="round"
          strokeLinejoin="round"
        />
        {/* Knob: circle that moves left/right */}
        <circle
          cx={checked ? 16 : 8}
          cy={12}
          r={3}
          fill={checked ? 'white' : 'transparent'}
          stroke={checked ? 'white' : 'currentColor'}
          strokeWidth={2}
          strokeLinecap="round"
          strokeLinejoin="round"
        />
      </svg>
      <span className="min-w-0 wrap-anywhere">{children}</span>
    </label>
  )
}
