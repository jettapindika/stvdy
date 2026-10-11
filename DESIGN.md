# Stvdy — Design Direction

> Status: **drafted by the agent from the spec**, with the honest caveat below.
> This is the soul document `antislop.md` asks for: identity, palette, type,
> mood, and the three dials. It is a filter input, not a rulebook.

**Honest caveat, as agreed before writing this.** Agent-chosen style drifts
toward default AI taste, which is exactly what antislop exists to filter. This
draft is derived from constraints the spec already fixes (who the user is, what
the product does, what the price implies), not invented from nothing, but the
first pass may read monotonous. Correct it rather than living with it. Nothing
here is load-bearing on a whim: each choice names its reason, so a wrong choice
is easy to spot and cheap to change.

---

## Who this is for

An Indonesian university student, mid-range Android phone and a laptop, paying
Rp10,000/month out of their own pocket. They use this for long study sessions on
material they must be examined on: reading sources, asking questions, drilling
flashcards, checking a plan before an exam.

What follows from that:

- **Reading-heavy, not glance-heavy.** Sessions are long and text-dense. Comfort
  over time beats impact on first look. This is the single biggest constraint and
  it rules out most "AI product" visual language.
- **Trust is the product.** The core promise is that an answer is traceable to
  the student's own material, page and all. Anything that makes a claim look
  firmer than it is (invented numbers, confident styling over uncertain data)
  damages the thing being sold.
- **Price-sensitive.** A student who paid Rp10,000 must not feel they bought a
  template. Craft is the value signal here, not decoration.

## Identity

**Study desk, not dashboard.** The product is built on documents, so the visual
language comes from paper and annotation rather than from control panels: warm
off-white stock, near-black ink, and a highlighter accent reserved for the
provenance layer.

**The motif is the citation.** The bracket label (`[1]`, `[W2]`) is the product's
core mechanic: it is how an answer points back at a page. It appears as a small
numbered mark that carries the highlighter accent, and it repeats everywhere a
claim is grounded (chat answers, flashcards, quiz explanations, report sections,
the printed cheatsheet). That is the one gesture that makes this design belong to
this product rather than to any chat app. It is also honest: the mark only ever
appears next to something genuinely traceable.

**What this is not.** Not a dark developer tool (the AI default for "app with
AI"), not a blue-purple gradient SaaS page, not a dashboard of stat cards. There
is no logo: the wordmark is set in type, because inventing a mark would be
fabrication.

## Palette

Two cores plus one accent, per R-29. Neutrals carry the layout; the accent is
rationed to the provenance moment and the primary action.

| Token | Light | Dark | Role |
|---|---|---|---|
| `paper` | `#fbf9f6` | `#1c1917` | Page ground. Warm, not white: less glare over long sessions, and it reads as stock rather than screen. |
| `surface` | `#f4f0eb` | `#262220` | Raised panels, inset fields. Separated by lightness, not by a shadow. |
| `ink` | `#241f1c` | `#f2ede7` | Body text. Warm near-black; pure `#000` is harsh against paper and looks unset. |
| `ink-muted` | `#6b625b` | `#a99f96` | Metadata, captions, labels. |
| `rule` | `#e2dcd4` | `#3a3431` | Hairlines and dividers. Deliberately low contrast: a border is structure, not content. |
| `mark` | `#f5c451` | `#f0b849` | **The accent.** Highlighter amber, used for citation marks and the primary action, nowhere else. |
| `mark-ink` | `#3a2708` | `#241a06` | Text on `mark`. |
| `ochre` | `#8a5a12` | `#e8b45c` | Accent as text or icon, where a filled `mark` block would be too loud. |
| `danger` | `#a02c2c` | `#e88a8a` | Errors only. |
| `success` | `#2f6b3f` | `#7fc08c` | Confirmations only. |

Why amber and not blue: blue is the default accent of every generated product,
and a study tool has a genuine alternative with meaning behind it. Highlighting is
what a student physically does to study material, so the accent borrows from the
behaviour the product supports. It also cannot be mistaken for a link, which
keeps "colored text" meaning exactly one thing.

Every text pair above passes WCAG AA in both modes (lowest is 5.63:1); verified
numerically, not by eye. The `rule` and `surface` separations are intentionally
under 1.5:1, since they are non-text boundaries.

## Typography

| Use | Face | Reason |
|---|---|---|
| UI and body | **IBM Plex Sans** | Humanist, designed for text at length, and unusually complete Latin-Extended coverage, which matters for Indonesian. Chosen over Inter (the model default) for the reason, not against it: Plex's slightly narrower, more calligraphic forms hold up better in the dense reading views this product is mostly made of. |
| Evidence layer | **IBM Plex Mono** | Citation labels, chunk ids, page numbers, timestamps, code. Monospace makes the provenance layer visually distinct from prose at a glance, so a student can tell "this is a pointer" from "this is an explanation" without reading. |
| Printed documents | Inter + JetBrains Mono | `templates/cheatsheet.v1.html` already ships this pair and is a separate artifact with its own print constraints. Deliberately not unified yet; unifying it is a print-quality decision, not a consistency chore. |

Scale: 1.25 ratio, `0.75 / 0.875 / 1 / 1.125 / 1.5 / 1.875 / 2.25rem`. Body 15px
rather than 16px, because the reading views are dense and 16px pushes line
length past comfortable.

## Mood and dials

**Dial: ENERGY 1 / RHYTHM 2 / MOTION 1.**

- **ENERGY 1.** A study tool used for hours. It should feel like a well-made desk,
  not a product announcement. Nothing shouts; hierarchy comes from space, weight,
  and the single accent.
- **RHYTHM 2.** Consistent enough to learn once, with deliberate breaks where the
  work genuinely differs: the three-panel workspace (sources, chat, studio) is a
  different composition from Today's single column, and the printed cheatsheet is
  a different world again. Uniformity would be a choice here, not an accident.
- **MOTION 1.** Hover, focus, and state transitions only. Two exceptions with a
  written purpose: streaming chat tokens appear progressively because that is the
  feature working, and progress indicators animate because a job is genuinely
  running. No entrance animations, no floating, no loops.

## Rules this direction commits to

- **No fabricated data, ever.** No stat cards with invented numbers, no delta
  badges without a real comparison period, no activity feed of people who do not
  exist. Screens with no data yet show an empty state that says why it is empty
  and what fills it. This is both an antislop rule (R-17, R-18, R-38) and the
  product's own promise about traceability.
- **The accent is rationed.** `mark` appears on citation marks and the single
  primary action per screen. If it starts appearing on badges, icons, and
  headings too, the identity is gone.
- **Structure over decoration.** Panels are separated by tone and hairlines, not
  by shadows or glow. Radius is a small set (4px controls, 8px panels, 999px only
  for the citation pill), so shape carries meaning: a pill is a citation mark or
  a count, never a button.
- **Dark mode is a peer, not a filter.** Both modes are designed; neither is the
  light theme with colors inverted. Dark is warm charcoal (`#1c1917`), not black
  and not blue-black, because the product is read at night.
- **Indonesian content is first-class.** Long Indonesian strings are the test
  case for every layout, not English. Copy is English for the MVP per the spec,
  but no component may assume short labels.
