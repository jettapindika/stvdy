# Stvdy — Figma redesign

**Source:** Figma file `loWSgyegOYiaEleabQOadJ` ("Random"), page `Stvdy` (node
`409:691`), section `424:2487` — 54 frames covering the design system and every
app screen in both viewports.

**Status of this document.** It is the record of a reversal. `docs/SPEC.md`
§ *Design system and UI rules* was the requirement of record and pinned its own
type scale and palette; the design now supersedes it. `docs/SPEC.md` and
`docs/DESIGN.md` are updated in the same change, and the reversal is recorded in
`docs/ASSUMPTIONS.md`. Anything this file does not name stays as it is.

---

## 1. What the design is

Three design-system sheets, then the product:

| Sheet | Node | What it fixes |
|---|---|---|
| Foundations | `411:17813` | Palette, dark core, type families and scale, spacing, radius, accessibility |
| UI controls | `411:18149` | Buttons (primary/secondary/ghost/danger) in four states each, inputs, validation, target sizes |
| Academic patterns | `411:18675` | Course cards, study-session cards, mastery, due states, citations, alerts, modal |

Then 51 product frames: Today (D/M), Workspaces (D/M), Workspace (D/M ×4), Study
workspace (light/dark), Flashcards (D/M), Quiz ×5, Planner ×4, Reports, Settings
×2, Pricing, Checkout, mobile plan ×2, auth ×4, and the states the app does not
have yet — command palette, Today offline, empty workspace, missing citations,
scanned-PDF error, admin dashboard.

### What must NOT be transcribed

The file contains a **foreign real-estate UI kit** nested inside component
instances. Its layers do not render in the frames but are present in the node
tree:

- `Poppins 22 w500` text "Buy" / "Rent" / "Sell" and a segmented control, inside
  the top bar's search component.
- Placeholder "Address, School, City, or Market".
- Inside every course card: "67 beds", "80 baths", "7000 sqft", "View Details".
- "Explore great neighborhoods", a `zt` avatar, and a 26px Poppins "zt" glyph.

None of it is built, and **Poppins is never introduced** — the three families
(Plus Jakarta Sans, Inter, JetBrains Mono) are fixed, bundled locally, and a
fourth would need bundling too (no CDN).

The `Stvdy / Study workspace / Light` and `/ Dark` frames are **annotated
component specimens**, not app screens: they open with "04 / LIGHT WORKSPACE ·
The component system in a real study context" and carry a third navigation
vocabulary (Workspace / My courses / Study sessions / Source library, user "Alex
Morgan"). They are used for component styling in context only; the app's
navigation comes from the Today/Workspaces frames and from `SPEC.md:664-665`,
which already prescribed it.

---

## 2. Authority and the conflicts it settles

`SPEC.md:664-665` already required the design's shell — *"Phone bottom tab bar,
5 tabs: Today, Workspaces, Study, Planner, More. Tasks live inside Planner as a
segmented control (Plan | Tasks)"* and *"Desktop/web sidebar … Today, Workspaces
(course list nested), Study, Planner (Calendar | Tasks), and Settings at the
bottom. Top bar: ⌘K/Ctrl+K search and command palette, sync indicator, credit
meter, avatar menu"*. The shipped app deviated; this change makes it match.

Where the design contradicts the rest of the spec, the design wins:

| Question | Spec / app | Design | Resolution |
|---|---|---|---|
| Type scale | Display 32/40, H1 24/32, H2 20/28 | Display 48/62, H1 32/42, H2 24/32, H3 20/26, H4 18/23 | Design |
| Type floor | "NOTHING below 12px" | metadata 11/18, tab label 10 | Design — the floor is gone |
| Body | 14/22 desktop, 16/24 mobile | 16/24 both | Design |
| Heading tracking | `-0.02em` | `0` | Design |
| Button shape | pill (`999`) | 8px (`CONTROL RADIUS / 8 PX`) | Design |
| Sidebar width | 248px, collapsible to 64px rail <1200px | 216px, no rail | **216px**, and the 64px rail is dropped — no frame shows it |
| Nav labels | Today, Workspaces, Calendar, Tasks, Settings | Today, Workspaces, Study, Planner, Settings | Design (also what `SPEC.md:664` required) |
| Nav icons | "No icons, deliberately" | 20px outline icons per item | Design — the no-icons rule is overturned |
| Button hover | `color-mix(primary 88%, bg)` | `#4F46E5` → `#3525CD` | Design |
| Palette | — | identical to the shipped tokens | **No change** |

The palette needs no work: `#FAF8FF #FFFFFF #F2F3FF #DDE0F0 #131B2E #464555
#4F46E5 #3525CD #E2DFFF #006591 #C9E6FF #005338 #D5F5E8 #92400E #FEF3C7
#BA1A1A #FFDAD6` and the dark core `#0B0F19 #111827 #263246 #F1F5F9` all already
match `styles.css` exactly. This is a layout, scale and component change.

---

## 3. Type scale

As Tailwind utilities. Weights are the design's.

| Utility | Size / line-height | Family / weight | Role |
|---|---|---|---|
| `text-display` | 48 / 62 | Plus Jakarta Sans 600 | Design-sheet display, marketing |
| `text-h1` | 32 / 42 | Plus Jakarta Sans 600 | Page title |
| `text-h2` | 24 / 32 | Plus Jakarta Sans 600 | Section |
| `text-h3` | 20 / 26 | Plus Jakarta Sans 600 | Widget heading |
| `text-h4` | 18 / 23 | Plus Jakarta Sans 600 | Card title |
| `text-body` | 16 / 24 | Inter 400 | Body, both viewports |
| `text-label` | 14 / 20 | Inter 500 | UI label, nav, button |
| `text-small` | 13 / 20 | Inter 400 | Secondary |
| `text-caption` | 12 / 18 | Inter 400 | Caption, helper text |
| `text-meta` | 11 / 18 | JetBrains Mono 400 | Eyebrows, counts, timestamps, keys |
| `text-meta` (tab) | 10 / 12 | Inter 400/600 | Phone tab-bar label only |

`text-meta` is uppercase with `letter-spacing: 0.04em` when used as an eyebrow;
the design writes metadata in caps throughout and the tracking is what makes an
11px mono line legible. Heading `letter-spacing` becomes `0`.

---

## 4. Shape, spacing, motion

- **Radius** unchanged: 8 controls, 12 cards, 16 sheets/modals, 999 chips. The
  design's `CONTROL RADIUS / 8 PX` confirms 8 for buttons, inputs and fields.
- **Spacing** on the 4px grid, from the design's scale: 8 tight groups, 12
  control padding, 16 card body, 24 related blocks, 32 section separation.
- **Borders** 1px in both themes; `.card` is `surface` + `border`, never a
  shadow. Elevation only for `.float`.
- **Motion** unchanged: 120/200/300ms ease-out, `prefers-reduced-motion`
  honoured.
- **Desktop frame**: content padding 32px, section gap 24px, page heading 100px
  tall, top bar 80px, sidebar 216px.

**No fixed pixel widths are transcribed as constants.** Every measurement above
is the design's proportion, built in `rem`/flex/grid, because the pass condition
measures a 32px root font size (200% text) and Indonesian strings ~25% longer
than English, and fails on undersized targets, clipped content and sideways page
scroll. The 216px sidebar, 176×44 nav items, 153×34 credit pill and 512×44 search
field become `rem` sizes and `min-*`/`max-*` constraints.

---

## 5. App shell

**Sidebar (216px, desktop)** — white, right hairline, 32px inline padding:
wordmark (28/35 PJS 600) → five nav items (176×44, radius 8, 20px icon + 12px gap
+ 14/20 Inter 500) → 1px rule → "THIS SEMESTER" (11 mono) + course links (code
11 mono indigo, name 14/20) → spacer → help note (icon + 12/18 copy) + footer
metadata.

The **active** nav item is an `#E2DFFF` fill with `#3525CD` icon and text, no
left bar, and the weight goes 400 → 500 so the state is not colour-only.

Icons are the design's outline set, drawn locally as one bundled SVG component
set (no CDN, no runtime fetch): `layout-dashboard`, `folders`, `book-open`,
`calendar`, `settings`, `circle-help`, `sparkles`, `file-text`, `search`,
`chevron-down`, `chevron-left`, `chevron-right`, `arrow-left`, `check`,
`circle-check`, `x`, `copy`, `bookmark`, `undo-2`, `ellipsis`, `flame`,
`list-checks`, `layers`, `globe`, `upload`, `link`, `play`, `presentation`,
`chart-no-axes-column`, `maximize-2`, `expand`.

**Top bar (new, 80px)** — white, bottom hairline, 32px inline padding: global
search field (magnifier + placeholder + `⌘K` badge, radius 8) and, on the right,
the credit pill (radius 999, `#F2F3FF`, sparkles icon, `450 credits` 11 mono, a
24×4 progress bar) + user name + 44px avatar circle (`#E2DFFF` fill, 11 mono
initials in `#3525CD`).

The design puts this bar at y=0 because it assumes a frameless window.
`SPEC.md:89` requires a **36px top drag region** with the macOS traffic-light
inset left and the Windows controls inset right; `src-tauri/tauri.conf.json` sets
no `decorations` and no `data-tauri-drag-region` exists anywhere in `app/src`.
The bar therefore carries the drag region and the platform insets, and it is
built to sit under native decorations until that window mode is enabled — stated
here so it is a decision rather than a discovery.

**Page header** — eyebrow (11 mono, uppercase, `#464555`) / h1 (32/42) /
description (16/24), with an optional right-hand 44px control (date picker,
primary action). Replaces the current h1+subtitle `PageHeader`.

**Page footer** — 1px rule, then 11 mono: timezone left, saved-state right.

**Phone** — app header 72px (wordmark / search icon / 44px avatar), credit row
54px, page heading with a full-width primary action, and a tab bar of 5 items
each 24px icon + 10px label, active tab `#F2F3FF` with `#3525CD` icon and 600
label. The 44px `min-height` and the five-label constraint (A189) are kept. The
design's iOS status bar is OS chrome and is not built.

---

## 6. Navigation and routing

Nav: **Today · Workspaces · Study · Planner · Settings**; phone short labels
Today / Workspaces / Study / Planner / More.

| Route | Change |
|---|---|
| `/` | Today (index), unchanged |
| `/today` | **New alias** — `notificationPath` returns `/today` and it is not registered today, so a cards-due reminder lands on NotFound. Fixed. |
| `/workspaces`, `/workspaces/:id` | unchanged |
| `/study` | **New** — the Study hub the design draws and `SPEC.md:568` requires: decks across workspaces with due badges, recent quizzes with scores, one "Review all due" action |
| `/study/:deckId` | unchanged |
| `/planner` | Planner, absorbing Calendar and Tasks as views: `?view=plan|calendar|tasks`, with `list|board` for tasks |
| `/calendar`, `/tasks` | **Kept as redirects** into `/planner?view=…`, so `NotificationRouter` and existing deep links keep working |
| `/quiz/:quizId`, `/reports/:id`, `/cheatsheets/:id`, `/syllabus`, `/syllabus/:draftId`, `/billing`, `/devices`, `/styleguide`, purchase routes | unchanged |
| `/settings` | Sectioned: Account / Study preferences / Calendar connections / Devices & sync / Appearance, absorbing `/devices` and the calendar `ConnectionsPanel`. Sub-section in the path: `/settings/:section`. |
| `/dashboard` | Kept, restyled |
| `/admin` | **New** — the Admin dashboard frame, behind the existing admin authorization. Its route is registered for every account and the *server* is the gate (`AdminUser` answers 404 to a non-admin, A7) — but a route nothing links to is a screen an operator has to be told about by hand, so **Settings renders the link for an account whose `is_admin` flag is set**. That flag is the account's own, it is display-only (every admin route re-reads it from the row), and it is the only way in: the layout walk reaches a screen by tapping it, so without the link the console was unreachable and therefore unmeasurable. |
| Command palette | **New** — `⌘K`/`Ctrl+K`, client-side over already-fetched lists plus navigation commands. No server search endpoint exists and none is added. |

**Honesty constraint on the palette and the new screens:** the design's data is
illustrative ("Rani", "450 credits", "4-day study streak", "ILLUSTRATIVE DATA /
NOT A LIVE ACCOUNT" on the admin frame). Layout, type, spacing and component
structure come from the frames; every number, name and streak comes from the API,
with the real empty state where data is absent.

**Honesty constraint on "mastery":** the Academic patterns sheet specifies a
Mastery family showing "Course mastery 32% / 72% / 100%" and "24 / 24 concepts
retained". `SPEC.md:75-76` allows only figures the app computes and forbids a
composite readiness score. It is therefore implemented as its three named parts —
topic coverage, quiz accuracy (last 14 days, or "no quizzes yet"), cards due —
never as one number.

---

## 7. Data the design needs and the app does not have

`workspaces` is `id, user_id, name, created_at, updated_at, deleted_at`
(`migrations/0007`). `WorkspaceView` is `id, name, created_at, updated_at,
source_count`. The design's sidebar course links and course cards need three
fields that do not exist:

| Field | Used by | Decision |
|---|---|---|
| `course_code` (`BIO 201`) | Sidebar course link, course card | **Added server-side** — nullable text, max 16 |
| `color` (0–5 index into the six course hues) | Course marker, card, sidebar | **Added server-side** — smallint, default assigned round-robin on create |
| `exam_date` | Course card "Next exam · 05 NOV 2026" | **Added server-side** — nullable date |

Deriving them client-side would mean inventing them, which the honesty rules
forbid. Migration `0038_workspace_course_fields.sql`, extended `WorkspaceView`,
`POST`/`PATCH /workspaces`, and the client query. The sidebar's course list and
the course cards render only what is present; a workspace with no code shows its
name alone.

`AUTUMN 2026` in the sidebar footer has **no data source** and is not invented:
that line shows the account's real timezone instead (`ASIA/JAKARTA ·
UTC+07:00`), which the app already has.

---

## 8. Dark mode

- 53 of the 54 frames are light-only. Every new surface is expressed in the
  existing tokens (`--bg`, `--surface`, `--surface-raised`, `--border`,
  `--text*`, `--primary*`, `--source*`, `--success/warning/danger*`) so the dark
  block keeps working, and the one dark frame is used to check the dark
  treatment of the new chrome. No literal `#FFFFFF` / `#F2F3FF` in a component.

**Correction, made after the fact.** The paragraph above describes the *structure*
of dark mode correctly and its *values* not at all: this pass left the dark block's
values where it found them, one step away from the design's, on the reading that
they were a deliberate accessibility choice. They were not. `411:19278` is a
**component specimen** — its own annotation reads "Supplied core + documented
proposed role colors" — and `get_variable_defs` on it returns the design's dark
values, which its nodes apply through `color/source/*`, `color/danger/*` and
`color/brand/*`. Every one of those values clears 4.5:1 on the surface and on its
own tint (8.20:1 to 12.72:1), so nothing was being protected by the substitutes.
The dark block now carries the design's values; A250 records the reversal and the
four tokens that remain derived.

## 9. States

The app already implements `EmptyState`, `LoadingState`, `ErrorState` and
`SkeletonRows` (`components/ui/States.tsx`). They are restyled into the new
idiom rather than left in the old one, and the four states the design adds —
Today offline, empty workspace, missing citations, scanned-PDF error — are built
from them.

## 10. Verification

- `app/src/tokens/contrast.test.ts` and `invariants.test.ts` move to the design's
  values and keep every existing assertion.
- `StyleguideScreen` is rebuilt from the three design-system frames, so the
  gallery is the design spec page.
- `app/src/features/a11y.test.tsx` (axe) and `scripts/measure-mobile.mjs` run at
  both viewports, both themes, 100% and 200% text; no undersized target, no
  clipped content, no sideways scroll.
- `app/src/features/phoneLayout.test.ts` and the layout tests are updated to the
  new shell.
- i18n key parity holds with new keys for every added string.
- **Every screen the redesign added has a surface in the shared walk**, and the walk
  reaches it by tapping the control a user would. Two of them did not, and closing that
  is what this round did:
  - **The admin console** had no surface *and* no route in — its only link is the
    Settings section an operator sees, which did not exist. The two were fixed together,
    and the surface is what proves the link renders.
  - **The task board** had no surface. `TasksScreen` renders the list by default and the
    board only for `?view=tasks&layout=board` on a non-phone, and nothing clicked the
    Board link — even though `LayoutSwitch`'s own comment says the links exist "so each
    layout is an address the student can come back to and the accessibility walk can
    reach". `tasks-board` is a **desktop-only** surface (a phone renders the card list for
    both layouts, so walking it there would measure the list twice and report a PASS for a
    screen it never saw), and the harness gained that one flag to express it.
- **Two surfaces remain unmeasured, and both are stated rather than implied.** Neither is
  a coverage claim:
  - **`/styleguide`** is a developer gallery with no route into it from the shell (it is
    reached by typing the URL), and the walk only reaches a surface by tapping. Its axe run
    is `StyleguideScreen.a11y.test.tsx`, so its contrast and semantics are checked and its
    **layout at 200% text is not**. It carries several fixed-count grids (`sm:grid-cols-2`,
    `sm:grid-cols-4`) whose text is a token name rather than student copy, which is why it
    was left; if it is ever linked from Settings it needs a surface like the console's.
  - **The command palette**, and this one was *tried*. `ONLY_SURFACES=palette` at 1365×900
    reported **450 problems** against a dialog behaving exactly as designed: every one was
    the scrim covering the page behind it, plus `372 trapping` and `4 without a ring` from
    the Tab leg — because a modal *does* cover the page and *does* trap focus. Those two
    criteria exist to catch a control a neighbour paints over and a focus order that
    escapes a page, so walking a modal reports its design as a defect, and teaching the
    harness about modals would weaken both checks for the other 30 surfaces. What the
    palette gets instead is axe over the combobox/listbox/option structure
    (`a11y.test.tsx`) and the arrow/Enter/scroll contract (`CommandPalette.test.tsx`); its
    **layout at 200% text is the one property nothing measures**.
- Fidelity is judged **per screen against its frame at the frame's own viewport**
  (1440×900 desktop, 390 phone) with the frame's data state. A dev account with
  no data renders the empty states, so each screen is also checked against a
  seeded account (`scripts/seed-demo.sh`, added by this change, driving the real
  API).

### The gate, as measured

Both legs were run against the built app with an explicit `OUTPUT`, so the numbers below
are reproducible from `var/` rather than only from a transcript:

```
OUTPUT=gate5-redesign-phone.json node scripts/measure-mobile.mjs
VIEWPORT_WIDTH=1365 VIEWPORT_HEIGHT=900 VIEWPORT_DPR=2 \
  OUTPUT=gate5-redesign-desktop.json node scripts/measure-mobile.mjs
```

| Leg | Surfaces | Combinations | Targets | Text pairs | Lowest contrast | Tab stops |
|---|---|---|---|---|---|---|
| 390×844, both themes, 100%/200% | 29 | 116 | 1,924 | 5,516 | 4.97:1 | 1,928, 0 unreached, 0 without a ring |
| 1365×900, both themes, 100%/200% | 30 | 120 | 2,556 | 7,772 | 4.79:1 | 2,472, 0 unreached, 0 without a ring |

Zero clipped, zero overlapped, and the page width exact at every one of the 236
combinations. The desktop leg is the one that had never run for any phase until this
change; it is also the only leg that can see the summary-card clipping above, and it walks
**one surface more than the phone does** — the task board, which a phone does not render.

The desktop leg's lowest pair is the board's own empty-column line, `text-subtle` on
`surface-raised` ("Nothing here yet.", 4.79:1 against the 4.5:1 body minimum). It is worth
naming because it is the *caption on a raised block* pair `tokens.ts` declares, and the
board is the first surface to render it — so the number moved from 4.97 to 4.79 when the
surface was added, and it moved because a declared pair finally got measured rather than
because anything regressed.

### Two instruments for the question the gate cannot answer

Gate 5 answers "is any target undersized, any text unreadable, anything clipped" — all of
which have numbers for answers. It cannot answer **"does this screen match its frame"**,
because the frames are 1440, 1680 and 1760 wide and the gate walks 390 and 1365. So the
comparison needs its own instruments, and two were added:

- **`scripts/screenshot-frames.mjs`** photographs each screen at *the frame's own width*,
  in the frame's own theme, with the frame's own data. It imports `measure-mobile.mjs`'s
  fixtures and its `signIn`/`openSurface` rather than restating them, so the screen it
  captures cannot drift from the screen the gate measures. `var/frames/` holds the
  captures and `var/frames/figma/` the frames they are compared against.
- **`scripts/probe-frame.mjs`** prints the browser's `getComputedStyle` and
  `getBoundingClientRect` for a screen's shell, its boxes and every text node. This is the
  one that finds *numbers*, because the frames' own values are exact — `get_metadata`
  returns every node's `x`, `y`, `width` and `height` — so "the rail's wordmark sits at
  y=28 with height 35" is checkable rather than eyeballed. It is how the 32px content
  padding, the 216px rail and the `176x44` nav item were confirmed.

Both are development instruments, not gates: they print, they do not pass or fail, and
neither is wired into CI.

---

## Defects the gate found, and what they were

Recorded because each was found by *running* the app rather than by reading it, and
because two of them had been shipped for a while without anyone noticing.

| Defect | Mechanism | Why nothing else caught it |
|---|---|---|
| The calendar — and therefore the Planner, which now hosts it — crashed to the router's error boundary for any account with an exam ahead of today | `CalendarScreen` built the date with ``new Date(`${goal.target_date}T12:00:00Z`)``, but `goals.target_date` is `timestamptz` (`migrations/0012`), so the value is already an instant: the template produced `2026-11-16T09:00:00ZT12:00:00Z`, an Invalid Date, and `Intl.DateTimeFormat.format` throws `RangeError` on one | The SPA fixture supplied a **date-only** `target_date`, which is a shape the server never sends. The suite was green against a contract that does not exist. Fixed at both ends: the template reads the instant, and the fixture now sends one |
| The shell scrolled the whole page sideways at **both** viewports, at 200% text | `.search-field` had `width: 100%` and no `min-width`, so its flex-basis floor was its *content* — icon + the full placeholder + the `⌘K` badge | The overflow is reported by an **ancestor**, not by the element that is too wide, which is the same mechanism the project documented for `min-w-60` and for the dashboard's `truncate`. Fixed with `min-width: 12rem` (a floor, not zero — zero let the field be squeezed to 71px and clip its own label) plus `flex-wrap` on the bar |
| A truncated course name in the sidebar was unreachable | The rail is 216px and a course name is longer; the span truncated with no way to read the whole value | The harness exempts a `truncate` **only when a `title` attribute carries the full text**, which is a rule about reachability rather than about layout. The `title` was missing |

Two more were found while fixing the above, both the same class — a value compared
against the wrong shape:

- `CalendarScreen` filtered the next exam with `goal.target_date >= todayKey`, a
  **string** comparison between a full instant and a date key. It now asks the
  question in the reader's local days, through the same `localDateKey` the grid uses
  for the same field.
- The card generator wrote `due_at` from the injected clock but left `created_at` at
  the column default, so a card's history had two different "start" instants. That is
  invisible until a replay empties the history — which is exactly what undo does.

### The second round, from the screens that had never been walked

The admin console was the one new screen with **no surface in the layout walk** —
`scripts/measure-mobile.mjs` had never listed it — and it was also the screen with no
route in: its only link is the Settings section an operator sees, which did not exist
until this round. Both were fixed together, and measuring it found two defects that
nothing else could have:

| Defect | Mechanism | Why nothing else caught it |
|---|---|---|
| The console's page container was **wider than its parent**, so the whole document scrolled sideways at 390px — 1714px against a 390px viewport — while three `overflow-x-auto` table boxes sat inside it containing nothing | The container is `mx-auto flex max-w-6xl`, and `mx-auto` sets **both cross-axis margins to `auto`**. A flex item with an auto cross margin is not stretched: it is sized by its content, so the container took its max-content width, which the tables' `min-w-3xl` made 1714px at 200% text. `w-full` is what makes the width definite and puts the scroll back inside the tables' own boxes | Every inner box was individually correct — each table *was* in an `overflow-x-auto` wrapper, which is exactly the fix the dashboard's defect had prescribed. The failure was one level up, in the ancestor that nobody reads because it looks like the standard container. Confirmed by setting `min-width: 0` on it in the browser (no effect) and `width: 390px` (1714 → 390) before changing any source |
| A card heading and its count were **clipped** at 200% text: "Every account" beside "2 accounts" measured 290px in a 212px card | `.widget-heading` was a flex row with no `flex-wrap`, so its two items could not go to separate lines. Wrapping puts the count below the title — a taller card, every figure still readable | The pair is inside a card, so the overflow was reported by the card rather than by either item, and no other surface's card heading is long enough for the sum to exceed its box at 32px root |
| The summary cards' figures were **clipped on desktop at 200% text**, which is the half of the gate the phone leg cannot see | The row was `sm:grid-cols-2 xl:grid-cols-4` — a fixed count — while at a 32px root the rail is **432px of a 1365px window** (it is 216px *in rems*, so it doubles with the text). The section was left 853px, four columns gave each card 189px, and `p-6` is 48px a side at that scale: 93px of content for a 64px-font rupiah figure that needs 162px. Now `repeat(auto-fit, minmax(9rem, 1fr))`, the same instrument the study screen's grade row uses: a rem floor grows with the root, so the count falls as the text grows — four across at 100% (the design's row) and two at 200% | The phone leg passed the same markup, because at 390px the grid is one column and every card is full width. A fixed column count is only ever wrong at *some* viewport-and-scale pair, and this one is wrong at the desktop pair the phone leg never measures. The floor is 9rem rather than the smallest value that fits because Indonesian copy runs ~25% longer and the margin was 11px |

**The reusable part is the first one.** A screen whose inner content is a wide table
needs its *outer* container to have a definite width, and `mx-auto` alone does not
give it one. Every other screen in the app that reaches for `overflow-x-auto` was
checked against this at the same time; the ones that were already correct carried
`w-full` on the container, which is why this is a missing class rather than a wrong
pattern.
