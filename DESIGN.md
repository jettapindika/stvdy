# Stvdy — Design Direction

> **Source: the user's own Figma design.** This document transcribes it; it does
> not invent it. Figma file `Random`, node `392-691`
> (`https://www.figma.com/design/loWSgyegOYiaEleabQOadJ/Random?node-id=392-691`),
> a Stvdy landing and auth page.
>
> Colours below were **sampled from the rendered frame**, not eyeballed. The one
> thing that could not be read from the image is the typeface (see Typography).

## Identity

**Cool, bright, and confident.** The design is a light, airy page on a cool
off-white ground, with pure white cards floating on it, near-black type, and one
azure blue carrying the brand. Nothing is warm, nothing is heavy, and there is a
lot of air: the hero headline is set large with tight leading, and everything else
gets out of its way.

**Blue is the identity; black is the action.** The design never uses blue for a
button. The primary action ("Try Stvdy", "Continue with email") is a **black
pill**, the brand blue is for the wordmark, links, and the illustration, and the
green is for a positive mark. That split is the most distinctive decision in the
design and the reason it does not read as a generic blue-button product, so it is
kept everywhere.

**The illustration is the brand asset.** A flat, geometric open book in azure,
with a four-point sparkle and a teal checkmark. Two colours, thick rounded
strokes, no gradients, no 3D. It is the only decorative element in the design and
it is specific to a study product, which is why it is reproduced rather than
replaced with an icon.

## Palette

Sampled from the frame. Two cores plus two functional colours, per R-29.

| Token | Light | Dark | Role |
|---|---|---|---|
| `paper` | `#f5f7fb` | `#12141a` | Page ground. Cool and slightly blue-tinted, not warm and not pure white. |
| `card` | `#ffffff` | `#1a1d24` | Panels and cards. Pure white in light mode, which is what lets a card read as raised on the tinted ground without needing a heavy border. |
| `surface-sunk` | `#eef1f7` | `#0e1015` | Inputs and insets: recessed rather than raised. |
| `ink` | `#101010` | `#f2f4f8` | Body text and the action fill. Near-black from the design, not `#000`. |
| `ink-muted` | `#5f6b7a` | `#9aa5b8` | Metadata and secondary text. |
| `rule` | `#dfe4ee` | `#2b3038` | Hairlines and borders. |
| `accent` | `#3588c8` | `#6db4e8` | **The brand azure**, sampled from the wordmark, book, and sparkle. Fills, illustration, and large text only. |
| `accent-text` | `#1a6aa8` | `#6db4e8` | The same hue darkened for body-size text. The design's blue is 3.56:1 on paper, which is AA only at large sizes, so links use this instead. |
| `success` | `#0e9080` | `#3fbfae` | The checkmark teal. Confirmation and positive state. |
| `success-text` | `#0c7f71` | `#3fbfae` | The same hue for body-size text. |
| `danger` | `#b3261e` | `#f2b8b5` | Errors only. Not in the design; borrowed from the same cool family so it does not clash. |
| `on-action` | `#ffffff` | `#12141a` | Text on the action fill. |

Verified in the rendered DOM: `ink` on `paper` 17.74:1, `accent-text` 5.33:1,
`success-text` 4.56:1, `ink-muted` 5.06:1, `on-action` on `ink` 19.03:1. The
brand `accent` at 3.56:1 is used only for fills, illustration, and text at 18px+
where AA-large applies.

## Typography

**Unresolved: the typeface could not be read from the render.** The headline is a
neutral grotesque with a double-storey `a` and a straight-tailed `y`; that is
consistent with Inter, Plex, or several others, and the resolution is not enough
to tell. Until the user confirms, the app uses **IBM Plex Sans** for a reason that
holds either way: it has complete Latin-Extended coverage, which matters for
Indonesian source material, and its narrower forms hold up in dense reading views.
**IBM Plex Mono** carries the evidence layer (page numbers, chunk ids, citation
labels, timestamps) so a pointer reads differently from prose at a glance.

Scale, from the design's proportions: hero `2.25rem/1.15`, page title
`1.5rem/1.25`, section `1.125rem`, body `0.9375rem/1.6`, meta `0.75rem`.

## Shape and elevation

- **Buttons are pills** (full radius). This is the design's most visible shape
  decision: both the primary black button and the secondary white button are
  fully rounded. It is a system choice, not a default, and it is applied
  consistently.
- **Cards are `14px`** with a soft shadow. The shadow is a real elevation marker
  here: the white auth card genuinely floats above the tinted ground, which is why
  it reads as raised rather than decorated.
- **Inputs and small controls are `8px`.** The pill is reserved for buttons and
  status chips so shape keeps meaning: rounded rectangle means "field", pill means
  "action or count".
- **No borders where a shadow or a tone shift will do.** The design separates the
  card from the ground with white-on-tint plus a shadow, not a hard outline.

## Dials

**ENERGY 2 / RHYTHM 2 / MOTION 1.**

The design is not the calmest setting (ENERGY 1: a large confident headline and a
black CTA that clearly says "act") nor the loudest. RHYTHM 2: the landing page has
one composition, and the app shell varies deliberately between the three-panel
workspace and single-column screens. MOTION 1: the design is static; transitions
are state changes only.

## Rules this direction commits to

- **Black is the action colour; blue is the identity colour.** Never a blue
  button. This is the design's most distinctive decision.
- **No fabricated data, ever.** The design shows a landing page, so it has no
  metrics to fake. The app inherits the stricter version of the same rule: no
  invented counts, deltas, or feeds (R-17, R-18, R-38), and empty states that name
  the cause and the next action (R-27).
- **The illustration appears where it belongs and nowhere else.** It is a brand
  asset for empty and entry surfaces, not a decorative sticker for every screen.
  In particular the sparkle is part of the illustration, not a feature icon: R-04
  forbids sparkles as generic iconography, and scattering them would be exactly
  that.
- **Air over density.** Generous whitespace is structural in this design. Padding
  is not tightened to fit more in; screens scroll instead.
- **Dark mode is derived, not inverted.** The design is light-only. Dark mode maps
  the same hues onto a cool near-black ground and inverts only the action fill
  (light background, dark text), keeping every contrast pair above AA.
- **Indonesian content is the layout test case**, not English: copy is English for
  the MVP, but no component may assume short labels.

## Not yet unified: the printed cheatsheet

`templates/cheatsheet.v1.html` predates this direction and uses its own palette
(Inter + JetBrains Mono, `--accent: #2563eb`, gray `[2]` citation spans, and a
semantic component palette: blue analogy, yellow formula, red trap, green
memorisation). That is a deliberate deferral, not an oversight: the cheatsheet is
a **print artifact** with its own constraints (ink coverage, A4 page breaks, KaTeX
metrics), and reconciling it with the app is Phase 6 work with a real print test
behind it. Until then, **the citation motif lives in the app only** and the
cheatsheet keeps its own citation styling. Recorded here so the design system of
record is not read as claiming a unification that does not exist.
