# Stvdy — Design Direction

> **Source of the values: the Figma design.** This document is the implementation
> record: what the tokens are, what each one means, what was measured, and which
> earlier decisions it replaces.
>
> **The design is the Figma file again, and this is the second reversal.** The
> history matters because both directions are recorded below and a later reader
> will find traces of each:
>
> 1. The first revision transcribed a landing/auth mock (`Figma file Random`, node
>    `392-691`) and sampled a palette from it.
> 2. The second revision replaced that wholesale with a palette and type scale the
>    spec fixed for itself, and this file then said the design was "no longer
>    sampled from a Figma frame".
> 3. **This revision returns to the design.** The Figma file's `Stvdy` page
>    (section `424:2487`, 54 frames) now covers the design system and every
>    screen in both viewports, and it is the requirement. `docs/SPEC.md` § Design
>    system and UI rules was rewritten in the same change; the decision record is
>    `docs/superpowers/specs/2026-10-12-figma-redesign-design.md`.
>
> What is genuinely new in the design, and what it overturns, is under
> **What the design changed** below. What it confirms — the palette, the three
> typefaces, the 4pt grid, the radius scale, the 12px-floor's *spirit* — is not
> restated here.

## Identity

**Calm, fast, and honest.** The audience is Indonesian university students on
mid-range Android phones, studying at night on unreliable campus Wi-Fi under exam
pressure. The interface is a light, low-contrast-ground page with white cards,
near-black type, and one indigo that carries every action. Nothing shouts; nothing
is animated for decoration.

**Indigo is the action.** `--primary` fills buttons, marks the selected item, and
carries the brand. This is a deliberate reversal of the earlier direction, which
made the action fill near-black and reserved blue for identity only. The spec
names indigo as the action colour, and an action colour that also appears on every
interactive control is the single most legible thing a student can learn about an
app in one session.

**Sky is sources only.** `--source` and `--source-tint` mark citations and
nothing else. A sky chip on screen means "this came from your material". That is
the one visual signal separating grounded text from generated text, so spending it
on anything else — an info banner, a link, a "beta" tag — destroys the distinction
the product is built on. Web-search results are deliberately *not* sky: they are
not the student's material, and they use the neutral raised surface instead.

**The illustration is the brand asset.** A flat, geometric open book with a
four-point sparkle and a checkmark, drawn in `currentColor` so it inherits the
theme. Two colours, thick rounded strokes, no gradients, no 3D. It belongs on
empty and entry surfaces and nowhere else; the sparkle is part of it, never a
standalone feature icon.

## Palette

Every value below is the spec's. The ratio beside each text token is **computed
from the token values by `app/src/tokens/contrast.test.ts`**, which fails the build
on a miss in either theme — so this table cannot drift from what renders.

| Token | Light | Dark | Role |
|---|---|---|---|
| `bg` | `#FAF8FF` | `#0B0F19` | Page ground. |
| `surface` | `#FFFFFF` | `#111827` | Cards and panels. |
| `surface-raised` | `#F2F3FF` | `#1B2435` | Insets and secondary blocks: raised, not recessed. |
| `border` | `#DDE0F0` | `#263246` | Hairlines and dividers. 1px in both themes. |
| `border-strong` | `#7E85A6` | `#6A789D` | A boundary that must be visible: input edges. Derived, see below. |
| `text` | `#131B2E` | `#F1F5F9` | Body text. 17.16:1 / 16.19:1 on `surface`. |
| `text-muted` | `#464555` | `#B4BDCF` | Secondary text. 9.36:1 / 9.39:1. |
| `text-subtle` | `#6B6A7C` | `#8493AA` | Captions and hints. 5.28:1 / 5.69:1. |
| `primary` | `#4F46E5` | `#4F46E5` | **The action fill.** White on it is 6.29:1 in both themes. |
| `primary-text` | `#3525CD` | `#C4BFFF` | Indigo as text: links, active nav. 9.14:1 / 10.34:1 on `surface`. |
| `primary-tint` | `#E2DFFF` | `#28234D` | Selected rows, working chips. |
| `source` | `#006591` | `#8CD3FF` | **Citations only.** 4.97:1 / 8.96:1 on its tint. |
| `source-tint` | `#C9E6FF` | `#0D2B40` | The citation chip's fill. |
| `success` | `#005338` | `#85E0BA` | Done and mastered. 7.87:1 / 8.22:1 on its tint. |
| `success-tint` | `#D5F5E8` | `#10382D` | |
| `warning` | `#92400E` | `#F8D787` | Due soon. 6.37:1 / 9.74:1 on its tint. |
| `warning-tint` | `#FEF3C7` | `#392C16` | |
| `danger` | `#BA1A1A` | `#FFB4AB` | Again, overdue, destructive, error. 5.00:1 / 8.20:1 on its tint. |
| `danger-tint` | `#FFDAD6` | `#432327` | |
| `ring` | `#4F46E5` | `#C4BFFF` | The focus ring. Derived, see below. |

Two tokens are derived rather than specified, and both for the same reason: the
spec's value cannot carry the role it is given.

- **`border-strong`.** `--border` is 1.25:1 on the ground and 1.31:1 on a card,
  which is exactly right for a divider and far short of the 3:1 WCAG 2.2 requires
  of a component boundary. An input whose only edge is a 1.25:1 hairline is an
  input a low-vision user cannot find. `#7E85A6` is the same hue family at 3.44:1
  on the ground and 3.63:1 on a card.
- **`ring`.** The spec's dark `--primary-text` (`#C4BFFF`) is the focus ring in
  dark mode, and the fill `--primary` in light. Using `--primary` in both would put
  a 2.82:1 ring on a dark card, which is a focus indicator that fails the same
  3:1 rule it exists to satisfy.

**Course colours** are six categorical hues — indigo, sky, emerald, amber, rose,
violet — each with a tint, each ≥ 4.5:1 both on the ground and on its own tint, in
both themes. They are used as a 4px marker plus a tinted chip plus an initial or
icon, and **never as the only signal**: two courses in the same colour are told
apart by their names, and a colour-blind user loses nothing. A course colour is not
a status colour; amber means "due soon" and a course is never "due".

## Typography

Bundled locally, never fetched (`scripts/fetch-fonts.mjs` writes both the woff2
files and `app/src/fonts.css`):

- **Plus Jakarta Sans** 600/700 — headings.
- **Inter** 400/500/600 — UI and body.
- **JetBrains Mono** 400/500 — page refs, timestamps, hotkeys. The evidence layer:
  a pointer into a source reads differently from prose at a glance.

`latin-ext` is kept for every family. Indonesian source material and romanised
names carry characters outside basic latin, and dropping the subset renders them
in a fallback face mid-sentence.

Scale, **from the design**, as Tailwind utilities:

| Utility | Size / line-height | Use |
|---|---|---|
| `text-display` | 48 / 62 | The design system's own sheets; marketing |
| `text-h1` | 32 / 42 | Page title |
| `text-h2` | 24 / 32 | Section |
| `text-h3` | 20 / 26 | Widget heading |
| `text-h4` | 18 / 23 | Card title |
| `text-body` | 16 / 24 | Body, **at every viewport** |
| `text-label` | 14 / 20 | A control's label, a row's primary line |
| `text-small` | 13 / 20 | Secondary |
| `text-caption` | 12 / 18 | Helper text |
| `text-meta` | 11 / 18 | Eyebrows, counts, timestamps — JetBrains Mono |
| `text-tab` | 10 / 12 | The phone tab-bar label only |

Two changes are reversals, and both were rules rather than values:

- **The 12px floor is withdrawn.** The design's metadata role is 11px monospace,
  and it is the most characteristic line in the redesign — every eyebrow, count,
  timestamp and keycap. It is uppercase with `letter-spacing: 0.04em` when it
  labels something (`.eyebrow`), which is what makes an 11px mono line legible.
  The spirit of the old rule survives: nothing is small *and* low-contrast, and
  every one of these pairs is measured by `contrast.test.ts`.
- **Body is 16/24 on desktop too**, up from 14/22. The old desktop body was a step
  smaller than a phone's for no reason other than that a desktop has more room,
  which is the wrong direction for a reading surface.

Heading `letter-spacing` is `0`, not the old `-0.02em`: the design measures every
heading at `ls 0.0`, and tight tracking on a 32px heading reads as cramped rather
than deliberate. Mobile inputs are 16px, because iOS zooms the whole page when a
focused field's text is smaller, leaving the student's view scrolled sideways
mid-sentence. Every layout must survive 200% text scaling and Indonesian strings
(~25% longer than English).

## Shape, elevation, motion

- **Radius:** 8 controls, 12 cards, 16 sheets and modals, 999 chips. Shape carries
  meaning: rounded rectangle means "field", pill means "action or count", 16px
  means "this layer floats".
- **1px borders in both themes.** A card is `surface` plus a `border` hairline, not
  a shadow. Elevation is reserved for things that genuinely float.
- **Shadow only on floating layers** (`.float`): popover, bottom sheet, modal,
  command palette.
- **Motion 120 / 200 / 300 ms, ease-out.** Named as `--duration-fast/base/slow`.
  `prefers-reduced-motion` turns the flashcard flip into a crossfade and stops
  every transition. No looping animation except a working indicator.

## Accessibility

WCAG 2.2 AA, checked in CI rather than asserted:

- **Focus ring on every interactive element**: 2px `--ring` with a 2px offset, in
  both themes, via `:focus-visible` so a mouse click does not leave a ring behind.
  An outline is never removed without this replacement.
- **Targets**: ≥ 44px on mobile (`.btn`, `.nav-item-tab`, and the interactive
  citation chip all carry a `min-height`), ≥ 32px with 8px gaps on desktop.
- **Never colour-only.** Every chip carries its word; the active nav item carries
  an indigo fill *and* a weight change (the design's item has no marker bar, so the
  weight is the non-colour signal); the course colour is always accompanied by a
  name or an initial.
- **Labels on all inputs**, accessible names on icon buttons, `aria-live` for
  toasts and streaming answers, one `h1` per screen.
- **Charts have text equivalents**; a heatmap is never the only place a fact lives.
- **Pinch-zoom is allowed**: no `user-scalable=no`, no `maximum-scale`.
- **Automated checks**: `app/src/tokens/contrast.test.ts` (every documented pair,
  both themes, plus a scan that fails on a raw hex in any component),
  `app/src/features/styleguide/StyleguideScreen.a11y.test.tsx` (axe on the
  component gallery), and `app/src/i18n/i18n.test.ts` (key parity, no unused copy,
  pseudo-locale expansion). `npm test` runs all three.

axe is a floor, not a certificate: it catches names, roles, labels and heading
order, and says nothing about whether a screen makes sense to a screen-reader
user. Its colour-contrast rules are disabled because jsdom does not apply the
stylesheet; the token test computes the real ratios from the real values instead.

## Honesty rules this direction commits to

The spec's § Honesty rules are a copy contract, and they constrain the design:

- **No invented numbers.** Every figure on screen is one the app computes: cards
  due, accuracy, topic coverage, minutes planned, streak, credits left. No match
  percentages, no confidence scores, no latency, no predicted grades, no
  "verified".
- **"Prep progress" shows its parts** — topic coverage, quiz accuracy (last 14
  days, or "no quizzes yet"), cards due. Never a composite "readiness".
- **The composer always carries** "AI can make mistakes. Check the citations."
  (`.composer-hint`). It is part of the composer, not a tooltip.
- **AI proposals are labelled "Suggested …"** and nothing is written before the
  user accepts. Never "Applied" for a pending proposal.
- **A generative action shows its credit cost before it runs.**
- **A locked feature's copy follows `config/billing_policy.toml`**: web and
  desktop may name the plan and link to checkout; iOS and Android show
  "Not available on your plan" with no price, no button, no link. `LockedFeature`
  implements both variants and is tested against both.
- **Empty states name the cause and the next action**, and never fake a feed.

## What the design changed

Beyond the palette, which the design already matched exactly, this revision
changed five things and confirmed the rest. Each was a decision the app had made
deliberately in an earlier revision, so each is a reversal rather than an update:

| The app had | The design has | Why it moved |
|---|---|---|
| No icons in the navigation, "deliberately" — five short labels were judged unambiguous and a thin-stroke set was judged to be the tell of a generated interface | A 20px outline glyph on every nav item, from one bundled set drawn for this product | The design's navigation item is an icon plus a label, and the design is the requirement. What survives from the earlier reasoning is the discipline: one family, drawn on a 24px grid, never an imported pack and never an emoji standing in for a state |
| No top bar at all; credits lived at the foot of the sidebar | An 80px top bar: a global search field that opens the command palette with its shortcut badge, the sync state, the credit pill, the account group | Every desktop frame carries it, and `SPEC.md:664` had required it all along — the app was the deviation |
| A 240px text-only rail | A 216px rail with the wordmark, five items, a "THIS SEMESTER" course list, a help note and the account's timezone | The course list is the design's; the term label ("AUTUMN 2026") has no data source and is not invented — that line shows the real timezone instead |
| Buttons were pills (`999`), so "pill" meant action-or-count and only the 8px rectangle meant "field" | Buttons are 8px (`CONTROL RADIUS / 8 PX`), and the pill is for chips and counts | The design separates the two roles differently: a control is a control. Hover darkens `#4F46E5` → `#3525CD` rather than mixing toward the page ground |
| Calendar and Tasks were two of the five nav destinations | Planner is one destination, and Calendar and Tasks are views inside it | `SPEC.md:664-665` had specified exactly this ("Planner (Calendar | Tasks)"). `/calendar` and `/tasks` survive as redirects, because notifications and bookmarks already name them |

**One rule in the design is not adopted, and the reason is the design's own.** Its
UI-controls sheet draws an input edge in `#DDE0F0`, which is 1.25:1 on the page
ground, while the same sheet states the target as "3:1 large text and control
boundaries". Those cannot both hold, and an input whose only edge is a 1.25:1
hairline is one a low-vision user cannot find. The stated rule wins, and it wins
for **both** controls the sheet draws that way: `.field` and `.btn-secondary` take
`--border-strong` (3.44:1 on the ground, 3.63:1 on a card). Every other surface
takes the design's `--border`.

The secondary button is the case worth stating, because its fill is `--surface` —
the same ground as the card it usually sits on — so the hairline was its *only*
boundary, and a low-vision user could not find the button. `contrast.test.ts`'s
`PAIRS` carries the pair for both, at the 3:1 UI floor, so the next person who
reaches for `--border` on a control fails the build rather than shipping it.

## What the design does not cover

- **Dark mode.** 53 of the 54 frames are light. Every surface is therefore
  expressed in the existing tokens, which follow the theme on their own, so a new
  screen works in dark without a `dark:` variant. There is no `dark:` in this
  codebase and there must not be one.
- **The loading, empty, error and offline states** the spec requires are in four
  frames (Today offline, Empty workspace, Missing citations, Scanned PDF error).
  The rest are built from `components/ui/States.tsx` in the same idiom.
- **The printed cheatsheet** (see below).
- **The window chrome.** The design's top bar sits at y=0 because it assumes a
  frameless window. `src-tauri/tauri.conf.json` sets no `decorations: false`, so
  the window is decorated and the OS title bar moves it. `.drag-region` exists and
  is zero-height; turning on a frameless window is a config change plus one class
  on the shell root, not a redesign of the bar.
- **A server-side search.** The command palette searches what the client already
  holds, and its empty state says so rather than implying a full-text index.

## Superseded

Recorded because the earlier revision committed to them and a later reader will
find traces:

| Earlier decision | Now |
|---|---|
| Palette sampled from the Figma frame: `paper #f5f7fb`, `card #ffffff`, `surface-sunk #eef1f7`, `ink #101010`, `rule #dfe4ee`, `accent #3588c8`, `success #0e9080`, `danger #b3261e` | The spec's palette, above. Every token renamed as well as revalued, so no component silently keeps the old colour. |
| "Blue is the identity, black is the action" — the primary button was near-black | Reversed: indigo is the action fill. |
| IBM Plex Sans + IBM Plex Mono, chosen because the Figma typeface was unreadable | Plus Jakarta Sans + Inter + JetBrains Mono, named by the spec. |
| Cards `14px` with a soft shadow as the elevation marker; dark mode lifted `card` and added a hairline to compensate | Cards `12px`, `surface` + 1px `border` in both themes. Elevation is for floating layers only. |
| Scale `hero 2.25rem/1.15`, body `0.9375rem/1.6`, meta `0.75rem` | The spec's scale, above. |
| "Copy is English for the MVP" | English **and** Bahasa Indonesia from day one, with a pseudo-locale test. |
| The styleguide page as a hand-maintained palette list | The same page, but its values are asserted by the contrast test rather than written in prose. |

| The spec's own type scale (Display 32/40 … Caption 12/16, "NOTHING below 12px") | The design's scale: Display 48/62, H1 32/42, H3 20/26, H4 18/23, Body 16/24, Label 14/20, Metadata 11/18 | Recorded above under **What the design changed**; the floor is withdrawn and desktop body rises to 16 |
| Desktop body 14/22 | 16/24 | The design has one body size for every viewport |
| Heading tracking `-0.02em` | `0` | The design measures every heading at `ls 0.0` |
| The Figma file as a source to be *rejected* ("the app's design is no longer sampled from a Figma frame") | The Figma file as the requirement of record | The file now covers the design system and every screen; the earlier judgement was about a landing mock, not about this |

Kept from every earlier direction, unchanged in spirit: the illustration as the only
decorative element, air over density, dark mode derived rather than inverted, the
refusal to invent data, and the palette — which the design already matched exactly,
token for token.

## Not yet unified: the printed cheatsheet

`templates/cheatsheet.v1.html` predates every revision of this direction and uses
its own palette (Inter + JetBrains Mono, `--accent: #2563eb`, gray `[2]` citation
spans, and a semantic component palette: blue analogy, yellow formula, red trap,
green memorisation). That remains a deliberate deferral: the cheatsheet is a
**print artifact** with its own constraints (ink coverage, A4 page breaks, KaTeX
metrics), and reconciling it with the app is Phase 6 work with a real print test.
