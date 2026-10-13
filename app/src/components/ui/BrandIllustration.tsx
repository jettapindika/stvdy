import { useTranslation } from 'react-i18next'

/**
 * The product's wordmark: the name, set in the heading face.
 *
 * **Text only, at 28px.** Every frame in the design draws the mark as a single text
 * node — 80x35 in the rail and the phone header, 80x36 in the auth header — with no
 * glyph beside it. This previously drew a 20px book mark plus a 16px label, which
 * was the illustration's own book reused as a logo; the frames do not do that, so
 * neither does this. `text-[28px]` is arbitrary rather than a scale step because the
 * type scale has no 28.
 */
export function Wordmark({ className }: { className?: string }) {
  const { t } = useTranslation()

  return (
    <span
      className={`font-heading text-[28px] leading-[normal] font-semibold text-text ${className ?? ''}`}
    >
      {t('app.name')}
    </span>
  )
}

/**
 * The brand illustration, reproduced from the user's Figma design.
 *
 * An open book in the brand azure with a four-point sparkle and a teal checkmark.
 * Flat, geometric, thick rounded strokes, two colours, no gradients. It is the
 * only decorative element in the design and it is specific to a study product,
 * which is why it is reproduced rather than replaced with a generic icon.
 *
 * Used where the design uses it: entry and empty surfaces. It is a brand asset,
 * not a sticker for every screen, and the sparkle stays inside the illustration
 * rather than becoming a feature icon (R-04 forbids sparkles as iconography).
 *
 * Colours come from the tokens, so it follows both themes without a second copy.
 *
 * Geometry note: the sparkle and the checkmark must sit clear of the book's
 * outline, as they do in the design. Overlapping them reads as a rendering bug
 * rather than a composition, so the book's box is kept inside x 56..344 and the
 * sparkle occupies the space above and to the right of it.
 */
export function BrandIllustration({ className }: { className?: string }) {
  const { t } = useTranslation()

  return (
    <svg
      viewBox="0 0 440 300"
      className={className}
      role="img"
      aria-label={t('brand.illustration.alt')}
      fill="none"
    >
      {/* The open book: two pages meeting at a spine. The top and bottom edges
          curve so the pages read as lifted rather than as a flat rectangle. */}
      <path
        d="M200 104 C164 84 116 79 76 90 L76 208 C116 197 164 202 200 222"
        stroke="var(--primary)"
        strokeWidth="12"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      <path
        d="M200 104 C236 84 284 79 324 90 L324 208 C284 197 236 202 200 222"
        stroke="var(--primary)"
        strokeWidth="12"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
      {/* Spine. */}
      <path
        d="M200 104 L200 222"
        stroke="var(--primary)"
        strokeWidth="12"
        strokeLinecap="round"
      />

      {/* Four-point sparkle, clear of the book's top-right corner. Concave sides,
          as in the design. */}
      <path
        d="M388 26 C392 54 404 66 432 70 C404 74 392 86 388 114 C384 86 372 74 344 70 C372 66 384 54 388 26 Z"
        fill="var(--primary)"
      />

      {/* Teal checkmark, inside the right page. */}
      <path
        d="M238 156 L266 186 L318 128"
        stroke="var(--success)"
        strokeWidth="16"
        strokeLinecap="round"
        strokeLinejoin="round"
      />
    </svg>
  )
}
