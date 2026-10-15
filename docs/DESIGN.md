# Stvdy — Design Direction

The current visual direction follows the request to restyle the whole app using
[Musicfy](https://github.com/realidkroo/musicfy) as a display and animation reference.
Stvdy keeps its study workflows, semantic colours, bundled typefaces and source
citations. Neutral surfaces, larger page titles, rounded panels and a floating
phone dock replace the earlier Figma-only surface treatment.

The Figma page `424:2487` remains the record of the information architecture and
screen content, not a requirement to retain the old palette or corner sizes.
Earlier implementation and verification records below are historical. The token,
type and motion sections describe the current implementation.

Reading this as a study workspace for university students, with a neutral,
content-first display: ENERGY 2 / RHYTHM 2 / MOTION 2.

## Identity

The audience is Indonesian university students on mid-range Android phones,
studying at night on unreliable campus Wi-Fi under exam pressure. Neutral page
grounds and flat cards keep long reading sessions quiet; larger titles and
rounded controls carry the requested display treatment. System appearance is the
default, with working Light and Dark choices rather than a forced dark theme.

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

The palette is shared by `styles.css` and `app/src/tokens/tokens.ts`.
`app/src/tokens/contrast.test.ts` computes the documented text and UI pairs in
both themes and checks that the stylesheet and token data agree.

| Token | Light | Dark | Role |
|---|---|---|---|
| `bg` | `#F5F5F7` | `#080809` | Neutral page ground. |
| `surface` | `#FFFFFF` | `#161618` | Cards and panels. |
| `surface-raised` | `#EBEBEF` | `#232326` | Insets and secondary blocks. |
| `border` | `#DCDCE3` | `#323237` | Hairlines and dividers. 1px in both themes. |
| `border-strong` | `#7E7E8E` | `#838390` | Visible input boundaries. |
| `text` | `#151518` | `#F7F7FA` | Body text. 18.22:1 / 16.90:1 on `surface`. |
| `text-muted` | `#4F4F5B` | `#C2C2CC` | Secondary text. 8.07:1 / 10.23:1 on `surface`. |
| `text-subtle` | `#686874` | `#A0A0AD` | Captions and hints. 5.50:1 / 6.99:1 on `surface`. |
| `primary` | `#4F46E5` | `#4F46E5` | Action fill; white labels clear 4.5:1 in both themes. |
| `primary-text` | `#3525CD` | `#C4BFFF` | Links and active navigation. 9.14:1 / 10.54:1 on `surface`. |
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

`border` separates surfaces; it is not the sole boundary of a field.
`border-strong` clears 3:1 against the page ground in both themes (3.67:1 light,
5.35:1 dark). Focus uses `ring`, with the light action colour and dark
`primary-text` colour tested against their adjacent surfaces.

**Course colours** are six categorical hues — indigo, sky, emerald, amber, rose,
violet — each with a tint, each ≥ 4.5:1 both on the ground and on its own tint, in
both themes. They are used as a 4px marker plus a tinted chip plus an initial or
icon, and **never as the only signal**: two courses in the same colour are told
apart by their names, and a colour-blind user loses nothing. A course colour is not
a status colour; amber means "due soon" and a course is never "due".

The light amber course pair is `#92400E` on `#FBEAB5`, keeping the tinted chip
distinct from the new neutral page ground while preserving readable text.

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

Current shared scale, as Tailwind utilities:

| Utility | Size / line-height | Use |
|---|---|---|
| `text-display` | 48 / 56 | Display specimens |
| `text-h1` | 40 / 52 | Page title and timer readout |
| `text-h2` | 24 / 32 | Section |
| `text-h3` | 20 / 26 | Widget heading |
| `text-h4` | 18 / 23 | Card title |
| `text-body` | 16 / 24 | Body, **at every viewport** |
| `text-label` | 14 / 20 | A control's label, a row's primary line |
| `text-small` | 13 / 20 | Secondary |
| `text-caption` | 12 / 18 | Helper text |
| `text-meta` | 11 / 18 | Eyebrows, counts, timestamps — JetBrains Mono |
| Phone dock label | 11 / 14 | Primary phone destinations |

Page headings reduce to 34/44 below 768px. The shared 40/52 title role leaves
enough height for the heading font at 200% text; the previous 40/48 line box
clipped it in Chromium. The timer caps at the title size and fits its card by
the rendered glyph count, including long resumed sessions, with 10% width
clearance and a 1.5 line-height for the mono numerals.

Two changes are reversals, and both were rules rather than values:

- **The 12px floor is withdrawn.** The design's metadata role is 11px monospace,
  and it is the most characteristic line in the redesign — every eyebrow, count,
  timestamp and keycap. Uppercase and untracked (`letter-spacing: 0`), it stays
  distinct from display headings without adding another visual component.
  The spirit of the old rule survives: nothing is small *and* low-contrast, and
  every one of these pairs is measured by `contrast.test.ts`.
- **Body is 16/24 on desktop too**, up from 14/22. The old desktop body was a step
  smaller than a phone's for no reason other than that a desktop has more room,
  which is the wrong direction for a reading surface.

Headings use `letter-spacing: -0.025em` for the compact display treatment;
evidence metadata remains untracked. Mobile inputs stay at 16px to avoid iOS
focus zoom. Layouts must survive 200% text scaling and Indonesian strings
(approximately 25% longer than English).

## Shape, elevation, motion

- Radius: 12px controls, 20px cards, 24px sheets and modals, pill-shaped navigation
  selections and chips. Fields and cards remain visibly different from navigation.
- Cards use a surface fill and 1px border. Shadow marks floating layers, including
  the phone dock, rather than every card.
- CSS feedback keeps the 120/200/300ms duration tokens. Motion 14.2.0 supplies one
  300ms opacity/10px route entrance and a shared spring selection in each primary
  navigation surface (stiffness 380, damping 34). No decorative loops are added.
- `MotionConfig reducedMotion="user"` and `useReducedMotion` disable route entry
  and selection movement when requested. Existing CSS reduced-motion rules still
  cover control transitions and the flashcard crossfade.

The route wrapper contains page content, not the command palette, so the palette's
fixed scrim stays viewport-sized. Auth cards reuse the same entrance. This replaces
the earlier CSS Blur Fade adaptation of Magic UI; there is no blur on reading
content and no remotely loaded animation code.

Settings opens as a category directory with a compact account summary. Account,
Study preferences, Calendar connections, Devices and sync, and Appearance each
have their own `/settings/:section` surface and a Back to Settings link. Wide
containers also show section navigation; narrow containers keep the form full
width. Study preferences includes notifications and language, preserving the
existing per-change saves and availability/plan restrictions.

Appearance boots from a validated `stvdy.theme` preference. A valid account setting
wins after refresh, including same-value refreshes. Explicit themes override all
colour/course tokens and `color-scheme`; System removes those inline overrides
and follows OS changes through CSS. Storage denial does not block rendering.

The persisted theme is a device-wide boot preference and intentionally survives
sign-out; the next account's valid preference replaces it after loading. The
palette initializes in the application module before React renders, not in an
inline HTML boot script. A slow module load may briefly show the OS palette;
native first-frame behaviour has not been measured.


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

### Responsive fidelity follow-up

- **Phone chrome:** a 72px wordmark/search/account header, safe-area padding added
  rather than substituted, and one full-width bottom navigation row. Its five tabs
  use the frame's 56px visual height with 8px surrounding padding and the real bottom
  safe area. The credit meter belongs beside the timezone inside Today, and in the
  directory's account-summary row, not in every phone header.
- **Sync:** settled devices no longer reserve a second sticky phone row. Pending
  writes, offline state, unreadable status, and server refusals still appear as a
  normal-flow notice linking to `/devices`; a refusal retains its server code and
  takes precedence over pending work. Desktop retains the compact top-bar control.
- **Today:** DOM order follows the phone reading order (review, capture, timeline,
  deadlines, timer, streak); explicit desktop grid placement preserves the frame's
  two-column arrangement. Timeline/deadline cards use 16px phone insets and 24px
  desktop insets. The desktop date control is not repeated in the phone introduction.
- **Workspace:** the desktop source/chat/studio proportions are 280:532:400; mobile
  workspace tabs remain segmented, while Studio section tabs are underlined. Source
  cards use 16px insets, phone workspace titles use 24/32, and source-kind choices
  reflow into readable grid cells instead of five squeezed text columns.
- **Study:** `/study/:deckId` uses the focused session frame without global chrome.
  A 760px desktop reading column, 350px phone card at the reference width, 24px/40px
  card insets, and 48px reveal/rating actions keep the question dominant. Both Back
  and End session remain real navigation; the source passage stacks when needed.
- **Quiz and Planner:** the quiz uses the full frame column with a 280px session
  rail; the planner's exam context is 288px at the reference viewport, and task
  columns use the frame's 644:128:96:80:184:148 proportions. Search is bounded rather
  than stretched across the task directory.
- **Reflow follows available text space:** named 64rem container thresholds switch
  Workspace, Quiz, study-source, and planner panels to their wide layouts. At 200%
  text or intermediate widths they stack/switch panels instead of shrinking their
  content into narrow strips. The tablet top-bar search floor is capped by its
  available width; its shortcut hint yields before the search action does.

The auth frames' Help/Privacy/recovery links remain absent until their real
destinations exist. No unpublished legal notice, fake control, review-duration
estimate, or Figma sample account data is introduced for visual completeness.


### Historical verification of the Figma responsive follow-up

`npm --prefix app test`: 75 files, 761 tests passed. Web and store builds pass;
`check:no-purchase-ui` finds no purchase path in the three shipped store files.
Vite still reports the existing large-entry-chunk warning; no warning threshold
was changed to suppress it.

| Browser layout run | Surfaces | Theme/text combinations | Lowest text contrast | Clipped / overlapped / undersized / missing focus rings |
|---|---:|---:|---:|---|
| Phone 390×844 | 31 | 124 | 4.71:1 | 0 / 0 / 0 / 0 |
| Desktop 1365×900 | 32 | 128 | 4.71:1 | 0 / 0 / 0 / 0 |

Both runs use light/dark and 100%/200% text. Reports are
`var/gate-fidelity-final-phone.json` and `var/gate-fidelity-final-desktop.json`.
Post-review rechecks cover the ten affected surfaces at both viewports, followed
by the three focused-review states after the final header change; all pass. These
filtered rechecks are not represented as additional whole-app runs.

Browser smoke additionally exercises all five phone destinations, account and
billing links, search/Escape, quiz submission with correct-answer feedback, and
the focused review's reveal/source/exit path. Page errors on those paths: zero.
The Quiz/Workspace width sweep covers 320, 390, 768, 1024, 1365 and 1440px at
100%/200% text with exact page width and readable upload choices. Reduced motion
reports no animation, transform or filter; ordinary route entry reports 200ms
and the shared ease-out curve. Route changes move focus to the main landmark.

Figma comparison captures are in `var/frames/fidelity-final/`. The quiz mapping
now photographs `/quiz/q1`, not its generation form. Each frame resets Today
fixtures before mounting, so a populated Today capture cannot change the data of
later planner/task captures. These are browser/fixture checks, not a native-device
or production-backend claim.

Delivery gate: Hard Gate PASS (measured reflow, contrast, targets and keyboard;
no invented controls or account content). Purpose Gate PASS (Figma-derived
palette, type, spacing, shapes and a single route-entry motion). Liveliness PASS
(ENERGY 1 / RHYTHM 2 / MOTION 2, review as the focal action). Craftsmanship PASS
(real routes and states retained, both themes checked, two code-review fix rounds
closed). No new visual assets or decorative loops were introduced.

### Musicfy redesign verification

The final frontend run passed 76 test files and 778 tests. Web and store builds
passed. The store-policy check found no purchase UI in the three shipped files;
the CDN check found no third-party loads. Vite retains its existing large-entry
chunk warning; the threshold was not raised.

| Browser run | Surfaces | Theme/text combinations | Lowest text contrast | Clipped / overlapped / undersized / missing focus rings |
|---|---:|---:|---:|---|
| Phone 390x844 | 36 | 144 | 4.71:1 | 0 / 0 / 0 / 0 |
| Desktop 1365x900 | 37 | 148 | 4.62:1 | 0 / 0 / 0 / 0 |

Reports: `var/musicfy-final-phone.json` and
`var/musicfy-final-desktop.json`. Both cover light/dark and 100%/200% text.
The phone run reached 1,940 targets and 1,924 keyboard stops; the desktop run
reached 2,672 targets and 2,612 stops. Neither found an unreached control or
an unintended focus trap.

Focused browser smoke exercised the Settings directory/detail paths, quiet-hour
clearing, channel toggling, language preference saves, all three appearance
choices against both OS palettes, route entry, reduced motion, and search/Escape.
The palette scrim measured exactly 1365x900 in that viewport. Ordinary route entry
was observed at opacity 0.0083 with a 9.9169px translation before settling;
reduced-motion entry rendered at opacity 1 with no transform. Page errors: zero.
Settings also held exact page width at 320, 390, 600, 768, 1024, 1365 and 1440px,
each at 100% and 200% text. Real persisted timer markers exercised 101, 1440 and
100000-minute readouts across four widths and two text scales: 24 cases, with no
row overflow or glyph clipping. The smoke marker was removed afterward.

These are fixture-backed browser/layout checks, not native-device or production
backend verification. The device-wide appearance preference and possible OS
palette flash before module evaluation are intentional tradeoffs recorded above.

#### Delivery gate

Scope: the redesigned shared surfaces and changed Settings/navigation controls;
unchanged backend and native integration behavior is not claimed as reverified.

| Item | Status and evidence |
|---|---|
| R-02 copy punctuation | PASS: new category descriptions and labels add no connector em dash; existing source titles and metadata remain data. |
| R-03 mobile reflow | PASS: the full phone gate and focused width sweeps found no page overflow or clipping. |
| R-17 statistics | PASS: balances, counts and plan names still read the existing API/query data. |
| R-18 testimonials | PASS: no testimonial, customer name or review block was added. |
| R-23 visual assets | PASS: bundled brand/icon assets and account initials are retained; no fabricated profile content was introduced. |
| R-24 navigation | PASS: directory routes, Back to Settings and primary destinations were exercised in the browser walks. |
| R-25 contrast | PASS: token tests and both full browser runs clear their text/UI thresholds. |
| R-26 controls | PASS: changed directory, theme, channel and form controls retain real routes or save handlers; browser smoke observed saves. |
| R-27 states | PASS: settings retry and unavailable-plan behavior are covered by the passing suite; existing loading/error/offline components remain. |
| R-28 FAQ | PASS: no FAQ was added or altered by the redesign. |
| R-32 keyboard | PASS: both full keyboard walks reached every expected control with a focus ring; search closes with Escape. |
| R-33 source implementation | PASS: styles, components and runtime integration live in source, not an external injection script. |
| R-34 themes | PASS: explicit Light/Dark beat the opposite OS palette; System followed both OS changes in Chromium. |
| R-35 runtime proof | PASS: tests, both builds, full browser walks and the changed-control smoke were run after the corrections. |
| R-36 claims | PASS: no security, compliance, customer or performance marketing claim was added. |
| R-37 direction | PASS: the user supplied Musicfy and approved the Settings direction before requesting the app-wide restyle. |
| R-38 content honesty | PASS: fixture rows exist only in verification; shipped views still consume real account/course data. |
| R-01 colour purpose | PASS: neutral surfaces support reading; the existing indigo action and source-only sky semantics remain. |
| R-04 icon purpose | PASS: category icons identify account, study, calendar, devices and appearance using the bundled set. |
| R-06 typography purpose | PASS: larger compact headings establish hierarchy; the 16px reading floor and evidence face remain. |
| R-07 background purpose | PASS: no grid, blueprint or decorative texture was added. |
| R-08 arrows | PASS: category chevrons and Back to Settings convey navigation direction, not decorative button emphasis. |
| R-09 badges | PASS: no promotional badge or duplicate pill eyebrow was added. |
| R-10 glass limit | PASS: the new surfaces and dock are solid fills without backdrop blur. |
| R-12 elevation purpose | PASS: card surfaces stay flat; the dock shadow identifies the floating navigation layer. |
| R-13 glow limit | PASS: no glow field or glowing control treatment was introduced. |
| R-14 card hierarchy | PASS: equal category cards are peer navigation choices, distinct from the compact account summary and detail forms. |
| R-19 motion purpose | PASS: one route entrance and shared navigation selection indicate state changes; reduced motion disables their movement. |
| R-22 illustrations | PASS: no unrelated illustration or stock asset was added. |
| Dials explicit | PASS: ENERGY 2 / RHYTHM 2 / MOTION 2 is recorded at the start of this document. |
| Dials reflected | PASS: the screenshots show bold page titles, quiet reading surfaces and bounded transitions rather than decorative loops. |
| Focal point | PASS: directory choices are the Settings focal task; each detail page groups the relevant controls. |
| Structural whitespace | PASS: summary, category grid and detail groups use separate spacing levels; mobile fields stack. |
| Deliberate accent | PASS: indigo marks actions and active navigation; source and status colours retain their existing semantic roles. |
| Identity motif | PASS: rounded panels and pill navigation repeat across the shared system without turning fields into pills. |
| Design read | PASS: the supplied reference and approved Settings direction establish the intended audience and display treatment. |
| C-1 intent | PASS: palette, type, shapes, layout and motion have written reasons in this document. |
| C-2 completeness | PASS: no new dead or placeholder control was shipped. |
| C-3 content composition | PASS: Settings is organised around actual preference categories, with no filler feature/stat grid. |
| C-4 resilience | PASS: theme/text/viewport checks, keyboard walks and error-state tests pass for the changed surfaces. |
| C-5 evidence | PASS: no invented user metric, testimonial or claim was introduced. |
| R-05 composition | PASS: directory, account summary and detail forms have different jobs and layouts. |
| R-11 shape consistency | PASS: controls/cards/sheets use 12/20/24px; navigation and chips use pills. |
| R-15 action labels | PASS: category names, Back to Settings and existing form actions describe their actual destinations or changes. |
| R-16 copy | PASS: new copy describes preferences and destinations without generic AI marketing language. |
| R-20 identity | PASS: Stvdy branding, study content, source semantics and evidence typography remain visible in both themes. |
| R-21 theme choice | PASS: System is the default and both explicit choices work; dark mode is not forced. |
| R-29 palette restraint | PASS: neutral grounds plus indigo dominate; categorical/status colours retain their documented roles. |
| R-30 reference adaptation | PASS: Musicfy informs surfaces and motion; its music content, assets and product identity were not copied. |
| R-31 decisions documented | PASS: the palette, type, shape, elevation, category layout and motion reasons are recorded above. |


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
| Cards `14px` with a soft shadow; later Figma cards `12px` | Cards `20px`, `surface` + 1px `border` in both themes. Elevation is for floating layers only. |
| Scale `hero 2.25rem/1.15`, body `0.9375rem/1.6`, meta `0.75rem` | The spec's scale, above. |
| "Copy is English for the MVP" | English **and** Bahasa Indonesia from day one, with a pseudo-locale test. |
| The styleguide page as a hand-maintained palette list | The same page, but its values are asserted by the contrast test rather than written in prose. |

| Earlier Display 32/40 and 48/62, H1 32/42 | Current Display 48/56, H1 40/52, with the reading and metadata roles retained |
| Desktop body 14/22 | 16/24 at every viewport |
| Figma heading tracking `0` | `-0.025em` for the requested Musicfy-inspired display treatment |
| Figma as the complete visual requirement | Figma information architecture with the current neutral surfaces, rounded shapes and motion described above |

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
