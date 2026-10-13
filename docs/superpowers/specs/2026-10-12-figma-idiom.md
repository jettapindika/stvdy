# The design idiom — how a screen is built after the redesign

Every screen follows this. It is the shared contract between the screens, and it
exists so that twenty screens do not become twenty dialects.

## The page skeleton

```tsx
<div className="mx-auto flex max-w-3xl flex-col gap-6 px-5 py-8">
  <PageHeader
    eyebrow={t('…')}        // 11 mono uppercase: a date, a course, a section
    title={t('…')}          // h1 32/42
    subtitle={t('…')}       // 16/24 muted
    action={<Link className="btn btn-primary">…</Link>}   // optional, one per screen
  />

  <Section title={t('…')} count={n}>   {/* h2 24/32, mono count */}
    <div className="card p-4">…</div>
  </Section>

  <PageFooter left={t('…')} right={t('…')} />   {/* 11 mono, timezone + saved state */}
</div>
```

- `max-w-3xl` is the reading column. `max-w-5xl` is for a screen with a sub-nav
  (Settings). A screen that owns its own width (Workspace, Study) says so itself.
- Content padding is `px-5 py-8`; the section gap is `gap-6`.
- **One primary action per screen.** It goes in `PageHeader`'s `action`, or at the
  top of the one card that owns it — never both.

## Cards and headings

- `.card` = `surface` + 1px `--border` + radius 12. Padding is `p-4` for a dense
  card and `p-6` for a dashboard widget. **Never a shadow** — elevation is
  `--float` only (popover, sheet, modal, palette).
- `.raised` = `--surface-raised` (#F2F3FF), radius 12. The "this is the priority"
  block and insets. It is a *tone*, not an elevation.
- A card with a heading uses `WidgetHeading`:
  ```tsx
  <WidgetHeading eyebrow={t('…')} title={t('…')} meta={t('…', { count })} />
  ```
  The heading is an `<h3>` at 20/26; `meta` is 11 mono and is **omitted** rather
  than zero-filled when there is nothing to count.
- A sub-card inside a card is `rounded-field border border-border p-3` at radius 8.

## Type utilities

| Utility | Use |
|---|---|
| `text-h1` | only via `PageHeader` |
| `text-h2` | only via `Section` |
| `text-h3` | only via `WidgetHeading` |
| `text-h4` | a card title inside a card |
| `text-body` | prose, an answer, a description |
| `text-label` | a control's label, a row's primary line |
| `text-small` | secondary line inside a card |
| `text-caption` | helper text under a field |
| `text-meta` | counts, timestamps, keys — monospace |
| `.eyebrow` | 11 mono, uppercase, tracked. Above a title or a card heading |

`.meta` and `.eyebrow` are the same size; `.eyebrow` is the uppercase tracked form.
Both are monospace, because metadata is the evidence layer.

## Controls

- `.btn` + one of `.btn-primary` / `.btn-secondary` / `.btn-quiet` / `.btn-danger`.
  Radius 8, height 44, weight 600. `.btn-primary` is the *one* primary action.
- `.field` for inputs, `select`s and `textarea`s. 16px text, `--border-strong`
  edge. Every field has a visible `<label>` — a placeholder is not a label.
- `.chip` + a tone (`.chip-ready`, `.chip-due`, `.chip-failed`, `.chip-working`,
  `.chip-source`) for a status. **The word is always present**; the tone repeats it.
- `.link` for a link inside prose. A link that is a *control* also needs
  `inline-flex min-h-11 items-center` (see the comment in `styles.css`).
- `.kbd` for a keycap. `.credit-pill` for the balance. `.avatar` for initials.

## Colour rules that are not negotiable

- **Indigo is the action.** `--primary` fills, `--primary-text` is indigo as text.
- **Sky is sources only.** `--source` / `--source-tint` mark a citation and nothing
  else. An info banner is not sky.
- `--success` done/mastered, `--warning` due soon, `--danger` overdue/destructive.
- **Never colour alone.** Every chip carries its word; every state has a non-colour
  signal (a weight change, an icon, a label).
- **No raw hex in a component** — `contrast.test.ts` fails the build on one. Every
  colour is a token, and the tokens follow the theme automatically, so a new
  surface works in dark mode without a `dark:` variant. There is no `dark:` in
  this codebase and there must not be one.

## Honesty rules this design must not break

- Every number on screen is one the API returned or the app computed from it.
  No percentages, no confidence scores, no predicted grades, no latency.
- A figure that is absent renders as an empty state, **never** as a zero, a dash
  or a placeholder number.
- "Prep progress" is its three named parts (topic coverage, quiz accuracy last 14
  days, cards due) — never one composite "readiness".
- An AI proposal is labelled "Suggested …" and nothing is applied before the
  student accepts. Never "Applied" for a pending proposal.
- A generative action shows its credit cost before it runs.
- The composer always carries "AI can make mistakes. Check the citations."
- Empty states name the cause and the next action, and never fake a feed.
- A locked feature's copy follows `config/billing_policy.toml`: web/desktop may
  name the plan and link to checkout; iOS/Android show "Not available on your
  plan" with no price, no button, no link.

## States

Every screen that fetches renders four states, from `components/ui/States.tsx`:
`LoadingState`/`SkeletonRows` while pending, `EmptyState` with a cause and an
action when the list is empty, `ApiErrorNotice` on failure, and an offline reason
where the action needs the network. `EmptyState` takes `title`, `reason` and an
optional `action`.

## Layout rules that the gates enforce

`scripts/measure-mobile.mjs` and `phoneLayout.test.ts` walk every surface at
390×844 and 1365×900, both themes, at 100% and 200% text, and fail on:

- a target under 44px on a phone (32px with 8px gaps on desktop);
- content clipped by its container;
- **sideways page scroll** — the most common defect, and the one to check first.

So: no fixed pixel widths for anything that holds text. Use `rem`, `min-w-0` on
flex children that contain a `truncate`, `flex-wrap` on any row of controls, and
`break-words`/`truncate` on a long string. A row of controls that cannot wrap is
a row that will overflow at 200% in Indonesian.

## i18n

- No hard-coded strings. Both `en.json` and `id.json`, same key order.
- A new key goes in **your own scratch file**, never directly in the locale files
  (a concurrent writer would lose your edit). Write
  `/tmp/omp-keys-<your-agent-name>.json` shaped like:
  ```json
  { "en": { "today": { "greeting": "Good morning" } },
    "id": { "today": { "greeting": "Selamat pagi" } } }
  ```
  Nested partial objects, only the keys you added, both languages. The parent
  merges them.
- Reuse an existing key when one already says the thing. `nav.*`, `actions.*` and
  `states.*` are shared.
- Indonesian is not a translation of the English sentence: it is the same fact in
  the register a Jakarta student uses. Short sentences. `kamu`, not `Anda`.
