#!/usr/bin/env node
/**
 * Gate 5 on a phone viewport, measured in a real browser.
 *
 * Roadmap gate 5 is "accessibility", and Phase 14's plan makes it literal: every
 * measured item is checked at **390x844**, and the >=44px interactive-target check
 * is measured in Chromium rather than asserted in vitest, because jsdom has no
 * layout engine and the repo's own `src/test/a11y.ts` says so. `getBoundingClientRect`
 * in jsdom returns zeros; there is nothing to assert.
 *
 * This is the same method Phases 6, 7, 8 and 9 used, and it is the method because
 * it has found real defects every time: Phase 6 found four (a report title rendered
 * 16px wide over 18 lines), Phase 7 a fifth (a 70x19 upgrade link against a 44px
 * target), Phase 8 a sixth (a fixed tab-bar band painted over the last line of a
 * paragraph at 200% text). None of them was visible by reading the code. So this
 * script measures and prints numbers; it does not look.
 *
 * Usage:
 *
 *   npm --prefix app run dev &          # or `npm --prefix app run preview`
 *   node scripts/measure-mobile.mjs
 *
 * `APP_URL` overrides the origin (default `http://localhost:5173`, the dev server
 * `vite.config.ts` pins). `CHROME_PATH` overrides the browser, falling back to the
 * same candidate list as `crates/server/src/render/chrome.rs` so the gate, the
 * server and the PDF acceptance test resolve the same binary by the same rule.
 *
 * **With no Chrome it fails loudly rather than skipping.** A check that silently
 * passes on a machine without a browser is the shape of coverage without the
 * substance, which is the same reasoning the PDF acceptance test's `#[ignore]` +
 * CI job follows.
 *
 * The API is answered from fixtures through request interception rather than by
 * running the real server, because what is under measurement is layout: a real
 * Postgres would add a way for this to fail that has nothing to do with the phone.
 * The session is established through the app's own sign-in form, so the shell, the
 * session store and the query cache are the real ones.
 */
import { execFileSync } from 'node:child_process'
import { existsSync } from 'node:fs'
import { createRequire } from 'node:module'
import { dirname, join } from 'node:path'
import { fileURLToPath, pathToFileURL } from 'node:url'

const root = join(dirname(fileURLToPath(import.meta.url)), '..')

/**
 * `puppeteer-core`, resolved from the app package rather than from this one.
 *
 * Every gate script lives in the repo's `scripts/`, and the browser dependency
 * belongs to `app/` (the plan puts it in `app/package.json` beside the app it
 * drives). Node resolves a bare specifier by walking up from the *importing
 * file*, which from here is the repo root and never reaches `app/node_modules`, so
 * the path is resolved explicitly instead of adding a second root manifest whose
 * only job would be to hold one devDependency.
 *
 * It is ESM-only (`"type": "module"`, one entry point), so the resolved path is
 * imported as a URL rather than `require`d.
 */
const appRequire = createRequire(join(root, 'app', 'package.json'))
const puppeteer = await import(pathToFileURL(appRequire.resolve('puppeteer-core')).href)

const APP_URL = process.env.APP_URL ?? 'http://localhost:5173'

/**
 * The phone, and the text scales gate 5 asks for.
 *
 * 390x844 is the viewport every phase's gate has used since Phase 6, so the
 * numbers here are comparable with the ones in those plans. `deviceScaleFactor: 3`
 * is a real phone's pixel ratio; it does not change CSS-pixel measurements, which
 * is what the 44px target is stated in.
 *
 * The three of them are overridable by environment variable **with these values as
 * the defaults**, because gate 5 has a second leg at desktop width and the same
 * checks apply to it: a `w-32` column or a `min-w-40` field is a defect on a 1365px
 * window too, just one that only shows at 200% text rather than at 100%. A second
 * script that copied this one to change a constant would be a second harness to
 * keep in step, and the two would disagree about what a defect is within a month.
 * The default is what keeps every existing phase's recorded numbers comparable: run
 * with no variables set and this measures exactly what it measured before.
 *
 *   VIEWPORT_WIDTH=1365 VIEWPORT_HEIGHT=900 VIEWPORT_DPR=2 SCALES=100,200 \
 *     OUTPUT=gate5-desktop.json node scripts/measure-mobile.mjs
 */
const VIEWPORT = {
  width: Number(process.env.VIEWPORT_WIDTH ?? 390),
  height: Number(process.env.VIEWPORT_HEIGHT ?? 844),
  deviceScaleFactor: Number(process.env.VIEWPORT_DPR ?? 3),
}
const SCALES = (process.env.SCALES ?? '100,200').split(',').map((value) => Number(value))

/**
 * A comma-separated subset of `SURFACES`, for working on one screen.
 *
 * The full walk is every surface x 2 themes x 2 scales with a Tab walk each, which is
 * minutes per iteration — and the surfaces are independent, so measuring one while
 * fixing it changes nothing about what the others would report. **A run with this set
 * is not a gate result**: the verdict line prints which surfaces it covered, so a
 * filtered run cannot be quoted as one. (No surface count here on purpose: the last
 * hard-coded one in this file went stale twice.)
 */
const ONLY = process.env.ONLY_SURFACES

/**
 * **The fixtures' date-bearing instants are anchored to now, not written down.**
 *
 * Several screens decide what to render by comparing a fixture's instant against the
 * *device* clock — `TodayScreen` and `calendar.ts`'s `localWeek` put an event in the
 * week containing now, `day.ts`'s `isOutstanding` reads `due_at`, and the pending
 * action card states when its proposal expires. An absolute fixture therefore has an
 * **expiry date**: it renders one state until that instant passes and a different one
 * afterwards, with no test failing — `calendar` would show the event in a later week or
 * not at all, and `today`'s buckets would move it from upcoming to overdue. The walk
 * would keep reporting PASS while measuring a different screen.
 *
 * That is not hypothetical: the same class went red on this tree twice on 2026-10-12 —
 * `CheckoutScreen.test.tsx` (a fixed `expires_at`) and `reminders.rs` (a fixed
 * `fire_at` against the server's registration window). `paymentRow()` below is the
 * in-file precedent for the remedy.
 *
 * `ANCHOR` is read once, at module load, so every fixture in one run agrees about what
 * "now" is even if the run crosses midnight.
 */
const ANCHOR = new Date()

/**
 * An ISO instant `days` from the anchor, at a fixed UTC time of day.
 *
 * The time of day is pinned rather than inherited from the run's clock because the app
 * renders these in the student's zone: a fixture whose instant drifted with the hour
 * would put a Jakarta date on either side of a day boundary depending on when the gate
 * ran.
 */
function daysFromAnchor(days, time = '09:00:00') {
  const date = new Date(ANCHOR)
  date.setUTCDate(date.getUTCDate() + days)
  return `${date.toISOString().slice(0, 10)}T${time}Z`
}

/**
 * The anchor's own week **in the student's zone**, `weekdayOffset` days from its
 * Monday, at a fixed time of day.
 *
 * Used for the calendar's one event. A plain "now + 2 days" is not enough: on a
 * Saturday that lands in the *next* week and the event leaves the visible window, so
 * the surface measures the empty grid.
 *
 * **And it has to be the student's week, not UTC's.** `localWeek(now, zone)` — what
 * the calendar actually renders — computes its Monday from `localDateKey(now, zone)`,
 * and the harness's account is `Asia/Jakarta` (+7). An earlier version of this helper
 * used `getUTCDay()`, which is a *different* week for the seven hours each Monday
 * where local has rolled over and UTC has not: the event landed on the previous
 * week's Wednesday, `localWeek` did not render that week, and the surface measured
 * the empty grid — with no `expect` to notice. That is the silent state-flip the
 * anchoring exists to prevent, so the frame here is the zone the screen uses.
 *
 * The emitted time is `09:00Z` deliberately: mid-day UTC is mid-afternoon in Jakarta,
 * so the instant's local date is the date this helper intends in any zone within ±9
 * hours of UTC.
 */
function thisWeek(weekdayOffset, time = '09:00:00') {
  const zone = USER.timezone
  // The student's calendar date, as YYYY-MM-DD — the same key `localDateKey` builds.
  const localDate = new Intl.DateTimeFormat('en-CA', {
    timeZone: zone,
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
  }).format(ANCHOR)
  // The weekday of that *calendar date*, read back through UTC so no zone shifts it.
  // `getUTCDay()` is 0=Sunday; the Monday-first index is `(day + 6) % 7`.
  const weekday = new Date(`${localDate}T00:00:00Z`).getUTCDay()
  const monday = new Date(`${localDate}T00:00:00Z`)
  monday.setUTCDate(monday.getUTCDate() - ((weekday + 6) % 7) + weekdayOffset)

  // **And it must not land on the anchor's own day.** `todayBuckets` puts an event on the
  // Today timeline whenever its local date *is* today's (`day.ts`'s `overlapsDay` filter,
  // rendered at `TodayScreen.tsx:154-172`), so a Wednesday run would give the `today`
  // surface the event's title and time — two more text pairs than a Tuesday run, for no
  // reason a reader could see. That makes every recorded total weekday-dependent, which is
  // the kind of drift that turns a re-measurement into an unexplained number change. One
  // step forward in the week costs nothing and the calendar still renders it: the offset is
  // 0-4, so +1 is at most 5, still inside the Monday-based week.
  const target = monday.toISOString().slice(0, 10)
  if (target === localDate) {
    monday.setUTCDate(monday.getUTCDate() + 1)
  }
  return `${monday.toISOString().slice(0, 10)}T${time}Z`
}

/** Where Chromium is looked for, the same list `render/chrome.rs` uses. */
const CHROME_CANDIDATES = [
  '/Applications/Google Chrome.app/Contents/MacOS/Google Chrome', // macOS
  '/usr/bin/google-chrome', // Debian/Ubuntu
  '/usr/bin/chromium',
  '/usr/bin/chromium-browser',
]

function findChrome() {
  const fromEnv = process.env.CHROME_PATH
  if (fromEnv !== undefined && fromEnv !== '') return fromEnv
  return CHROME_CANDIDATES.find((path) => existsSync(path)) ?? null
}

/**
 * The smallest interactive-target side the platform's rule allows.
 *
 * **This is viewport-keyed, and it has to be.** 44px is the *touch* target — R-03 and
 * WCAG 2.2 SC 2.5.8, which is about a pointer that lands somewhere on a 9mm finger —
 * and the spec's desktop floor is 32px. The repo states the relationship itself in
 * `.btn`'s comment: "44px minimum touch target, which also clears the spec's 32px
 * desktop floor".
 *
 * Applying 44 at 1365px flagged 200 desktop sidebar links at 215x40, which are
 * `min-height: 2.5rem` **by design** — so an undersized count at desktop width read as
 * a defect when it was the phone rule measured at the wrong width. The desktop leg's
 * genuinely undersized set is 12, not 224.
 *
 * The boundary is the `md` breakpoint (768px), the same one `useFormFactor` and the
 * CSS use for the sidebar/tab-bar swap: below it the layout is the touch one, above it
 * the pointer one. A third convention for "is this a phone" would be the defect this
 * avoids.
 */
const TOUCH_TARGET = 44
const DESKTOP_TARGET = 32
const TOUCH_MAX_WIDTH = 767

const MIN_TARGET = VIEWPORT.width <= TOUCH_MAX_WIDTH ? TOUCH_TARGET : DESKTOP_TARGET

/** WCAG 2.2 AA: 4.5:1 for body text, 3:1 for large text and UI graphics. */
const MIN_TEXT = 4.5
const MIN_LARGE = 3

/** The fixture account: a free plan, which is the state the plan screen exists for. */
const USER = {
  id: 'u1',
  email: 'student@example.com',
  email_verified: true,
  email_verified_at: '2026-10-11T08:00:00Z',
  plan_id: 'free',
  timezone: 'Asia/Jakarta',
  student_verified: false,
  // An administrator, because the admin console has no other way in: its only link is
  // the Settings section that renders for an operator, and the walk reaches a screen by
  // tapping it rather than by URL. So this flag is what makes the console *reachable*,
  // which is the precondition for measuring it at all.
  is_admin: true,
  created_at: '2026-10-11T08:00:00Z',
}

const USAGE = {
  plan: 'free',
  name: 'Free',
  subscription: { status: 'active', period_end: '2026-11-11T00:00:00Z' },
  credits: { monthly: 60, purchased: 12, total: 72 },
  storage_bytes: 1048576,
  limits: {
    workspaces: { limit: 2, used: 2 },
    sources_total: { limit: 20, used: 3 },
    max_file_mb: { limit: 20, used: null },
    storage_mb: { limit: 200, used: null },
    devices_max: { limit: 2, used: 1 },
    credits_per_month: { limit: 60, used: null },
    chat_msgs_per_day: { limit: 20, used: 4 },
    web_searches_per_day: { limit: 3, used: 3 },
    flashcards_per_month: { limit: 30, used: 12 },
    quizzes_per_month: { limit: 2, used: 0 },
    reports_per_month: { limit: 1, used: 1 },
    syllabus_imports_total: { limit: 1, used: 0 },
    google_accounts_max: { limit: 0, used: null },
  },
  features: {
    export_formats: ['md'],
    task_ai_breakdown: false,
    quick_capture: false,
    google_calendar: false,
    planner: 'manual',
    agent_actions: false,
    reminder_channels: ['web', 'push'],
    model_tier: 'cheapest',
    byok: false,
    advanced_analytics: false,
  },
  costs: {
    chat_message: 1,
    web_search: 2,
    flashcard_batch_10: 4,
    quiz_10: 6,
    report_outline: 3,
    report_section: 6,
    task_breakdown: 2,
    syllabus_import: 8,
    plan_generation: 2,
    agent_turn: 2,
    long_doc_threshold_tokens: 20000,
    long_doc_credits_per_1k_tokens: 1,
  },
  provider_cost: { paused: false, resets_at: '2026-11-01T07:00:00Z' },
}

const SETTINGS_ROW = {
  study_start_hour: 8,
  study_end_hour: 22,
  max_study_min_per_day: 240,
  block_len_min: 50,
  break_min: 10,
  reminder_channels: ['web', 'push'],
  quiet_start_hour: null,
  quiet_end_hour: null,
  language: 'en',
  theme: 'system',
  updated_at: '2026-10-11T08:00:00Z',
}

/**
 * One task, so the Tasks screen renders a row and the row's controls are measured.
 *
 * A list with nothing in it would measure the empty state, and the controls that
 * matter on that screen (the checkbox, the title link, the delete button) only
 * exist once there is a row. The task is Indonesian and long on purpose: a string
 * that fits is the case that proves nothing.
 */
const TASK = {
  id: 't1',
  workspace_id: null,
  parent_id: null,
  title: 'Selesaikan tugas kriptografi untuk kuliah hari Jumat',
  notes: null,
  // Four days out: far enough that the Today view does not put it in today's buckets,
  // near enough that the tasks row renders a date rather than "overdue". Anchored, so
  // the state cannot flip when this instant passes (see `daysFromAnchor`).
  due_at: daysFromAnchor(4, '16:59:00'),
  estimate_min: 60,
  priority: 2,
  status: 'todo',
  position: 0,
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
  deleted_at: null,
}

/**
 * One event, so the calendar's grid renders a block and the block's controls are
 * measured. An empty week measures the empty state, and the controls that matter
 * on that screen only exist once an event is in the window. The title is
 * Indonesian and long on purpose: a string that fits proves nothing.
 */
const EVENT = {
  id: 'e1',
  workspace_id: null,
  kind: 'study',
  title: 'Belajar kriptografi untuk ujian hari Jumat',
  // The week's own Wednesday, so the block is inside the Monday-based week the
  // calendar renders on any day the gate runs (see `thisWeek`).
  starts_at: thisWeek(2, '09:00:00'),
  ends_at: thisWeek(2, '10:00:00'),
  source: 'manual',
  task_id: null,
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
}

/** One calendar connection, so the connections panel renders a row rather than its empty state. */
const CONNECTION = {
  id: 'c1',
  provider: 'ics',
  account_email: null,
  status: 'ok',
  error: null,
  last_synced_at: '2026-10-11T08:00:00Z',
  study_calendar_id: null,
}

/**
 * One workspace, with material in it.
 *
 * The workspace screen is five of the surfaces below, and every one of them would
 * measure an empty state if the workspace had nothing in it: the panel strip, the
 * source rows, the chat thread and the three Studio forms all only exist once
 * there is something to render. The name is Indonesian and long on purpose — a
 * string that fits proves nothing, and this one has to survive a list row at 200%
 * text beside two buttons and a count.
 */
const WORKSPACE = {
  id: 'w1',
  name: 'Kriptografi dan Keamanan Informasi Terapan',
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
  source_count: 2,
  // The course fields the redesign added. The card and the workspace heading both
  // render them, and the walk is the only place the *combination* is measured: a
  // code, a count and a date on one mono line at 200% text.
  course_code: 'IF4020',
  color: 2,
  exam_date: '2026-10-26',
}

/**
 * Two sources, both ready.
 *
 * Ready rather than processing on purpose: a ready source is the only one the
 * Studio pickers will offer, so a pending one would leave every Studio form
 * measuring its "no indexed sources" hint instead of its controls. The titles are
 * Indonesian and long for the same reason as the workspace's.
 */
const SOURCES = [
  {
    id: 's1',
    workspace_id: 'w1',
    kind: 'pdf',
    title: 'Bab 4 — Algoritma Kunci Publik dan Tanda Tangan Digital',
    url: null,
    file_size: 1048576,
    mime: 'application/pdf',
    status: 'ready',
    error: null,
    created_at: '2026-10-11T08:00:00Z',
  },
  {
    id: 's2',
    workspace_id: 'w1',
    kind: 'pptx',
    title: 'Kuliah 7 — Fungsi Hash dan Integritas Pesan',
    url: null,
    file_size: 524288,
    mime: 'application/vnd.openxmlformats-officedocument.presentationml.presentation',
    status: 'ready',
    error: null,
    created_at: '2026-10-11T08:00:00Z',
  },
]

/**
 * A thread with both roles in it.
 *
 * The assistant turn carries a citation marker and the chunk it resolves to, so
 * the answer's chips and the "based on N sources" line are on screen and are
 * measured — they are links, and a link is a target. An empty thread would
 * measure the empty state, which is the one state on that screen with no controls
 * in it at all.
 */
const MESSAGES = [
  {
    id: 'm1',
    workspace_id: 'w1',
    role: 'user',
    content: 'Jelaskan bagaimana tanda tangan digital memverifikasi keaslian dokumen.',
    citations: [],
    used_web: false,
    created_at: '2026-10-11T08:00:00Z',
  },
  {
    id: 'm2',
    workspace_id: 'w1',
    role: 'assistant',
    content:
      'Tanda tangan digital memakai kunci privat penanda tangan untuk mengenkripsi ringkasan dokumen, lalu penerima memverifikasinya dengan kunci publik [1].',
    citations: [{ type: 'chunk', chunk_id: 'k1', source_id: 's1', page: 12 }],
    used_web: false,
    created_at: '2026-10-11T08:00:01Z',
  },
]

/** One ready deck, so the Studio flashcards panel renders a row rather than its empty state. */
const DECK = {
  id: 'd1',
  workspace_id: 'w1',
  title: 'Kriptografi Kunci Publik — Kartu Ulangan',
  source_ids: ['s1'],
  status: 'ready',
  error: null,
  requested_count: 10,
  card_count: 10,
  created_at: '2026-10-11T08:00:00Z',
}

/**
 * The study screen's two reads, and the cards behind them.
 *
 * Two cards rather than one, because the progress line is the surface's only
 * *status* and `Card 1 of 2` is the shape that shows it; and the first card
 * carries a `chunk_id`, because the revealed state's "Show source" button exists
 * only for a card that links to one. A card without a chunk would measure a
 * screen whose densest state is unreachable.
 *
 * `due_at` is in the past of the `now` the due fixture answers with, because
 * `dueCards` filters on it: a card due in the future is dropped from the queue
 * and the surface would measure the empty state instead of the card.
 */
const CARDS = [
  {
    id: 'c1',
    deck_id: 'd1',
    front: 'Apa yang dimaksud dengan kunci publik?',
    back: 'Kunci yang boleh diketahui siapa saja dan dipakai untuk memverifikasi tanda tangan.',
    chunk_id: 'k1',
    ease: 2.5,
    interval_days: 0,
    due_at: '2026-10-10T09:00:00Z',
  },
  {
    id: 'c2',
    deck_id: 'd1',
    front: 'Apa fungsi tanda tangan digital?',
    back: 'Menjamin keaslian dan integritas pesan, bukan kerahasiaannya.',
    chunk_id: null,
    ease: 2.5,
    interval_days: 0,
    due_at: '2026-10-10T09:00:00Z',
  },
]

/** The passage `c1` came from, for the revealed state's source panel. */
const CHUNK = {
  id: 'k1',
  source_id: 's1',
  page: 42,
  idx: 7,
  content:
    'Kunci publik adalah pasangan kunci yang boleh disebarkan. Kunci privat yang sesuai ' +
    'tetap dirahasiakan pemiliknya, dan hanya tanda tangan yang dibuat dengan kunci ' +
    'privat itu yang dapat diverifikasi memakai kunci publiknya.',
}

/** One ready quiz, for the same reason as the deck. */
const QUIZ = {
  id: 'q1',
  workspace_id: 'w1',
  title: 'Kuis Kriptografi Kunci Publik dan Fungsi Hash',
  difficulty: 'medium',
  question_types: 'mixed',
  source_ids: ['s1', 's2'],
  status: 'ready',
  error: null,
  requested_count: 10,
  question_count: 10,
  created_at: '2026-10-11T08:00:00Z',
}

/** One ready report, with an outline, so the row renders its count and its link. */
const REPORT = {
  id: 'r1',
  workspace_id: 'w1',
  title: 'Ringkasan Kriptografi untuk Ujian Akhir Semester',
  status: 'ready',
  error: null,
  outline: {
    title: 'Ringkasan Kriptografi untuk Ujian Akhir Semester',
    sections: [
      { heading: 'Kriptografi kunci publik', purpose: 'Dasar', labels: [1], gaps: false },
      { heading: 'Fungsi hash', purpose: 'Integritas', labels: [2], gaps: false },
    ],
  },
  body_md: '# Ringkasan\n\nIsi.',
  sections: [
    { heading: 'Kriptografi kunci publik', body_md: 'Isi.', generating: false, error: null },
    { heading: 'Fungsi hash', body_md: 'Isi.', generating: false, error: null },
  ],
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
}

/**
 * Two proposals waiting for the student, so the agent card is measured rather than
 * only its absence.
 *
 * Both tools, because they stress different things: `create_task` renders the
 * card's five-field `<dl>` (a long Indonesian title and notes, a `due_at` the card
 * formats, and the longest sentence on the card) while `create_flashcards` is the
 * one whose price the matrix states, so it is the only one that renders
 * `agent.card.cost`. Supplied by the Phase 12 slice that owns the screen.
 */
const PENDING_ACTIONS = [
  {
    id: 'a1',
    tool: 'create_task',
    args: {
      title: 'Selesaikan laporan kriptografi untuk Jumat',
      notes: 'Sertakan analisis fungsi hash dan tanda tangan digital pada bab empat.',
      due_at: daysFromAnchor(4, '16:59:00'),
      estimate_min: 90,
      priority: 1,
    },
    status: 'pending',
    created_at: '2026-10-12T08:00:00Z',
    // Tomorrow, so the card is live rather than expired, and anchored so that stays
    // true on every later run.
    expires_at: daysFromAnchor(1, '08:00:00'),
    resolved_at: null,
    resolved_by_device_id: null,
    result: null,
    error: null,
    message_id: 'm2',
  },
  {
    id: 'a2',
    tool: 'create_flashcards',
    args: { title: 'Kartu Ulangan Kriptografi Kunci Publik', count: 10, source_ids: ['s1', 's2'] },
    status: 'pending',
    created_at: '2026-10-12T08:00:05Z',
    expires_at: daysFromAnchor(1, '08:00:05'),
    resolved_at: null,
    resolved_by_device_id: null,
    result: null,
    error: null,
    message_id: 'm2',
  },
]

/**
 * The planner's and syllabus screens' fixtures (Phase 11).
 *
 * **These exist to close Phase 11's gate-5 leg (e)**, which had never been measured
 * because the walk could not *reach* the screens: neither nav has a planner item, and
 * the only two inbound links formed a closed cycle (`PlannerScreen` → `/syllabus` from
 * its empty-goals state, `SyllabusReviewScreen` → `/planner` after a confirm). The
 * `tasks` screen now renders the reciprocal half of the spec's `Plan | Tasks` control,
 * so `/planner` is reachable by tapping and the walk has a path.
 *
 * **One goal is enough to bring in the densest state**, because `PlannerScreen` does
 * `rows.find(g => g.id === selected) ?? rows[0] ?? null` — with one goal and nothing
 * selected, it becomes `active` and `PrepProgress` renders without a click. That is
 * what makes `/goals/:id/progress` a required fixture rather than an optional one.
 */
const GOAL = {
  id: 'g1',
  workspace_id: 'w1',
  title: 'UTS Kriptografi',
  target_date: daysFromAnchor(30),
  source_ids: ['s1'],
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
}

/**
 * `GET /goals/:id/progress`, as the PrepProgress card renders it.
 *
 * Every number here is the spec's own honesty rule in fixture form (A252): four honest
 * figures and **no composite**. `quiz_accuracy_pct` is a real percentage rather than
 * null, so the card renders its number branch — a null would render "no quizzes yet",
 * which is the *other* state and not the one leg (e) needs to measure for clipping.
 */
const GOAL_PROGRESS = {
  goal_id: 'g1',
  window_days: 14,
  topic_coverage_pct: 62,
  quiz_accuracy_pct: 74,
  cards_due: 27,
  weak_topics: ['Algoritma Kunci Publik'],
  topics: [
    { name: 'Fungsi Hash', accuracy: 0.79 },
    { name: 'Algoritma Kunci Publik', accuracy: 0.375 },
  ],
}


/**
 * A confirmed-ready syllabus draft, for the review screen.
 *
 * `status: 'ready'` because that is the state that renders the items to review; a
 * `processing` draft renders the waiting panel, which has no items and would measure a
 * fraction of the screen. The item indices are the contract's own — the position in the
 * **reply**, not in a sorted list (`crates/core/src/generation/syllabus.rs:109-113`),
 * which is the premise Phase 11's own suite got wrong five times.
 */
const SYLLABUS_DRAFT = {
  // `draft_id` and `source_id` are required by `DraftBody`; `course` renders the header.
  draft_id: 'd1',
  source_id: 's1',
  status: 'ready',
  error: null,
  course: { name: 'Kriptografi', code: 'IF4020', instructor: 'Dr. Sari' },
  // **The item shape is the contract, and getting it wrong is silent.** `reviewRows`
  // (`features/syllabus/review.ts:87-91`) drops any row whose `kind` is not one of
  // `['class','deadline','exam']` *before* it reads anything else, and requires a numeric
  // `index` and a non-empty `evidence`. The date field is **`when`**, not `starts_at` or
  // `due_at`. An earlier version of this fixture used `kind: 'event'`/`'task'` with
  // `starts_at`/`due_at`, which renders an **empty list** — and the surface would have
  // reported PASS while measuring a screen with no items in it, which is the exact failure
  // the `expect` on the review surface exists to catch.
  items: [
    {
      index: 0,
      kind: 'exam',
      title: 'UTS Kriptografi',
      evidence: 'Ujian Tengah Semester: 2 November 2026, 90 menit',
      needs_check: false,
      when: daysFromAnchor(21),
      rrule: null,
      start_time: null,
      end_time: null,
      topics: ['Fungsi Hash', 'Algoritma Kunci Publik'],
      weight_percent: 30,
    },
    {
      index: 1,
      kind: 'deadline',
      title: 'Tugas 4 dikumpulkan minggu depan',
      evidence: 'Tugas 4 dikumpulkan minggu depan',
      needs_check: true,
      when: daysFromAnchor(7),
      rrule: null,
      start_time: null,
      end_time: null,
      topics: [],
      weight_percent: null,
    },
  ],
}

/**
 * Phase 13's four screens, and the one thing about them a fixture has to get right.
 *
 * The pricing page, the billing page, the checkout screen and the plan screen are the
 * surfaces this phase adds, and the checkout screen is the one that needs care: **its state
 * is derived from the payment row**, so measuring it once measures one of six states and
 * says nothing about the other five. So the fixture below is *mutable* and each checkout
 * surface sets it before it walks, which is how the walk reaches `paid`, `expired`,
 * `review`, `failed` and `offline` as well as `waiting`.
 *
 * `offline` is the one that is not a body: the screen's own state when the request never
 * arrived, which is why `checkoutStateFixture` flips a flag the request handler reads and
 * *aborts* the poll rather than answering it. A 500 would be `waiting` with an error notice
 * and would measure the wrong screen.
 *
 * The six states are asserted **in words** below (`words` on each surface), because gate
 * 5(d) is that no state is carried by colour alone — and a screen whose whole subject is
 * whether money moved is the likeliest place for a status that is only a chip's tone.
 */
const PAYMENT_ID = 'p1'
const CHECKOUT = { status: 'pending', expiresInMs: 9 * 60 * 1000, offline: false, id: PAYMENT_ID }

/**
 * Switches the checkout fixture, and asserts the state name it was given.
 *
 * **Each state gets its own payment id**, and that is load-bearing rather than tidy: the
 * screen's poll is a TanStack query keyed by the id, and two surfaces that navigate to the
 * *same* URL do not remount the screen — so the second one would render the first one's
 * cached row and the walk would measure `waiting` six times while reporting six states. A
 * distinct id is a distinct query, fetched fresh, which is the only way a fixture can drive
 * a state machine from outside.
 */
function checkoutStateFixture(state) {
  const known = {
    waiting: { status: 'pending', expiresInMs: 9 * 60 * 1000, offline: false },
    paid: { status: 'paid', expiresInMs: 9 * 60 * 1000, offline: false },
    expired: { status: 'pending', expiresInMs: -60 * 1000, offline: false },
    review: { status: 'review', expiresInMs: 9 * 60 * 1000, offline: false },
    failed: { status: 'failed', expiresInMs: 9 * 60 * 1000, offline: false },
    offline: { status: 'pending', expiresInMs: 9 * 60 * 1000, offline: true },
  }
  const next = known[state]
  if (next === undefined) fail(`unknown checkout state ${state}`)
  Object.assign(CHECKOUT, next, { id: `p-${state}` })
}

/**
 * One payment row, as `POST /billing/checkout` and the poll both send it.
 *
 * The numbers do **not** follow from each other on purpose (`amount_idr` 5,555 against a
 * list price of 12,345 at 50% off): a screen that re-derived the total would render 6,172,
 * and the assertion in the per-screen test is that it renders the server's number. Here
 * they only have to be a plausible row for the layout to measure.
 */
function paymentRow() {
  return {
    id: CHECKOUT.id,
    order_id: 'stvdy_01j0checkoutexample',
    kind: 'subscription',
    plan_id: 'pro',
    billing_period: 'monthly',
    credits: null,
    amount_idr: 5555,
    amount_before_discount_idr: 12345,
    discount_percent: 50,
    discount_code: null,
    student_discount: true,
    qr_string: 'STUBQRIS:stvdy_01j0checkoutexample:5555',
    expires_at: new Date(Date.now() + CHECKOUT.expiresInMs).toISOString(),
    status: CHECKOUT.status,
    paid_at: CHECKOUT.status === 'paid' ? new Date().toISOString() : null,
  }
}

/**
 * `GET /plans`, with the prices present.
 *
 * A **student** account (`student_verified: true`), because that is the state the pricing
 * page's banner renders in and the one with the larger numbers to fit — a screen measured
 * with the banner absent would not measure the sentence that is hardest to hold.
 *
 * This fixture is also what the existing `plan` surface needed: `/billing` renders
 * `BillingScreen`, which calls this route, and with no fixture the walk was measuring that
 * panel's 404 notice rather than its content.
 */
const PLANS_VIEW = {
  plans: [
    {
      id: 'free',
      name: 'Free',
      limits: { workspaces: 1, sources_total: 3, max_file_mb: 20, devices_max: 1 },
      student_percent_off: 0,
      byok: false,
      credits_per_month: 60,
      price_idr_monthly: 0,
      price_idr_yearly: 0,
      student_price_idr_monthly: 0,
      student_price_idr_yearly: 0,
    },
    {
      id: 'pro',
      name: 'Pro',
      limits: { workspaces: 5, sources_total: 50, max_file_mb: 50, devices_max: 3 },
      student_percent_off: 50,
      byok: false,
      credits_per_month: 500,
      price_idr_monthly: 10000,
      price_idr_yearly: 100000,
      student_price_idr_monthly: 5000,
      student_price_idr_yearly: 50000,
    },
    {
      id: 'power',
      name: 'Power',
      limits: { workspaces: 'unlimited', sources_total: 500, max_file_mb: 100, devices_max: 10 },
      student_percent_off: 50,
      byok: true,
      credits_per_month: 2000,
      price_idr_monthly: 25000,
      price_idr_yearly: 250000,
      student_price_idr_monthly: 12500,
      student_price_idr_yearly: 125000,
    },
  ],
  credit_packs: [
    { credits: 100, price_idr: 3000 },
    { credits: 500, price_idr: 12500 },
    { credits: 1000, price_idr: 22000 },
  ],
  show_prices: true,
  purchase_allowed: true,
  currency: 'IDR',
  current_plan: 'free',
  student_verified: true,
}

/** `GET /billing/invoices`, with one row, so the history panel renders rather than its empty state. */
const INVOICES_VIEW = {
  invoices: [
    {
      id: 'p0',
      order_id: 'stvdy_01j0earlier',
      kind: 'credit_pack',
      plan_id: null,
      billing_period: null,
      credits: 500,
      amount_idr: 12500,
      amount_before_discount_idr: 12500,
      discount_percent: 0,
      discount_code: null,
      student_discount: false,
      qr_string: null,
      expires_at: '2026-10-01T08:15:00Z',
      status: 'paid',
      paid_at: '2026-10-01T08:04:00Z',
    },
  ],
}

/**
 * The four taps that reach the checkout screen, shared by its six state surfaces.
 *
 * "Choose Pro" is `pricing.choose` rendered with the plan's name; the click drives the real
 * `POST /billing/checkout` against the fixture, and the screen navigates to
 * `/billing/checkout/<the id the fixture returned>` — so the surface measures the route the
 * app actually takes rather than a URL typed into the address bar.
 */
const CHECKOUT_STEPS = [
  { nav: 'More' },
  { link: 'Open plan and usage' },
  { button: 'See plans' },
  { button: 'Choose Pro' },
]

/**
 * The way a phone reaches the study screen: the workspace, its Studio panel, and the
 * deck row's own "Study" link.
 *
 * Named once because four surfaces walk it — the card's unrevealed, revealed, source-open
 * and graded-by-key states — and a path copied four times is four places for it to drift.
 */
const STUDY_STEPS = [
  { nav: 'Work' },
  { link: WORKSPACE.name },
  { tab: 'Studio' },
  { link: 'Study' },
]

/**
 * The way an operator reaches the admin console: Settings, then the link the account
 * section renders for an administrator.
 *
 * **Two taps, and both are asserted by the walk rather than assumed.** Settings is where
 * the console's only route in lives, so if that link stops rendering — a renamed i18n key,
 * a `is_admin` that stops arriving on `/auth/me` — this surface fails with "no link Open
 * the admin console" rather than measuring a screen nobody can get to.
 */
const ADMIN_STEPS = [{ nav: 'More' }, { link: 'Open the admin console' }]

/**
 * The surfaces this walks, by the taps a user takes to reach them.
 *
 * **One table for both viewports** — it was written for the phone and the redesign's
 * desktop leg walks the same list, which is why the phone's short nav labels are aliased
 * to the sidebar's in `openSurface` rather than duplicated here.
 *
 * A surface may carry **`desktopOnly`**, and one does: the task board, which a phone does
 * not render at all (it draws the card list for both layouts). Walking it at 390px would
 * measure the list a second time and report a PASS for a screen the leg never saw — the
 * vacuity the other guards in this file exist to prevent — so it is dropped from the
 * phone leg and from that leg's surface count.
 *
 * The tab bar's five labels are the five destinations, and `/billing` is reached
 * the way a phone reaches it — through Settings — which is also the assertion that
 * it is reachable at all: a screen nothing links to would otherwise be measured by
 * URL and would pass while being invisible to a user.
 *
 * The five workspace surfaces are reached the same way, and each one re-walks its
 * whole path from the tab bar rather than continuing from wherever the previous
 * surface left the page: a surface whose steps only work in the order the list
 * happens to be written is a surface that silently measures the wrong screen the
 * first time the list is reordered. The workspace screen is the one screen Phase 14
 * left uncovered, and its panels are where the `min-w-40` this walk was written for
 * lives (`features/studio/QuizPanel.tsx`) — the panel strip only exists below `lg`,
 * so a phone is the only viewport that renders it at all.
 */
const SURFACES = [
  // **These three carry `expect` because their content is anchored to the run's clock.**
  // Without it a wrong anchor is undetectable: the surface renders a *different state* —
  // an empty week, an empty list — the walk reports fewer targets with zero problems, and
  // the run still passes. Each string can only appear in the intended state.
  //
  // `calendar`'s is the calendar's own count, which is the strongest of the three: it
  // proves the event landed inside the week `localWeek` renders, not merely that some
  // text is on the page.
  { name: 'today', expect: 'Due flashcards', steps: [{ nav: 'Today' }] },
  { name: 'workspaces', steps: [{ nav: 'Work' }] },
  // Calendar and Tasks are **views inside the planner** as of the redesign, so they are
  // reached through the Planner tab and its own switcher rather than as tabs of their own.
  // The `expect` strings are unchanged, which is what makes this a re-route rather than a
  // rewrite: the same content has to appear at the new address, or the move broke it.
  {
    name: 'calendar',
    expect: '1 event in this view',
    steps: [{ nav: 'Planner' }, { link: 'Calendar' }],
  },
  {
    name: 'tasks',
    expect: 'Selesaikan tugas kriptografi untuk kuliah hari Jumat',
    steps: [{ nav: 'Planner' }, { link: 'Tasks' }],
  },
  // --- The board, which is the second layout of the same screen and had no surface ----
  //
  // `TasksScreen` renders the list by default and `TaskBoard` only for
  // `?view=tasks&layout=board` on a non-phone (`TasksScreen.tsx`'s `layout === 'board' &&
  // !isPhone`), and nothing clicked the Board link — even though `LayoutSwitch`'s own
  // comment says the links exist "so each layout is an address the student can come back
  // to and the accessibility walk can reach". That promise was unkept: the board is a
  // three-column grid of cards with its own target sizes and its own headings, and it was
  // measured nowhere.
  //
  // **`desktopOnly`, and that flag is the honest form of the constraint rather than a
  // convenience.** A phone renders the card list for both layouts, so a phone walk of this
  // surface would measure the list a second time and report a PASS for a screen it never
  // saw — the vacuity every other guard in this file exists to prevent.
  //
  // The guard is the effort line, which is board-only copy and comes from the fixture's
  // `estimate_min`: the list has no equivalent, so a Board click that landed without the
  // layout changing leaves the string absent and fails rather than measuring the list twice.
  {
    name: 'tasks-board',
    expect: '1h · total estimated effort',
    desktopOnly: true,
    steps: [{ nav: 'Planner' }, { link: 'Tasks' }, { link: 'Board' }],
  },
  // --- The command palette is deliberately NOT a surface, and the reason is measured ----
  //
  // It was tried (`ONLY_SURFACES=palette`, desktop, 1365×900) and the walk reported **450
  // problems** against a dialog that is behaving exactly as designed: every one was the
  // scrim covering the page behind it (`overlapped … by div.fixed.inset-0.z-50`) plus
  // `372 trapping` and `4 without a ring` from the Tab leg, because a modal *does* cover
  // the page and *does* trap focus. Those two criteria are page-shaped — they exist to
  // catch a control a neighbour paints over, and a focus order that escapes a page — so
  // walking a modal reports its design as a defect, and teaching the harness about modals
  // would weaken both checks for the other 30 surfaces.
  //
  // What the palette gets instead is `a11y.test.tsx`, which renders it and runs axe over
  // the combobox/listbox/option structure this redesign added — the thing no other check
  // covers — plus `CommandPalette.test.tsx` for the arrow/Enter/scroll contract. Its
  // layout at 200% text is the one property that remains unmeasured, and that is stated in
  // the design doc's §10 rather than left implicit.
  { name: 'settings', steps: [{ nav: 'More' }] },
  { name: 'plan', steps: [{ nav: 'More' }, { link: 'Open plan and usage' }] },
  // --- Phase 11: the planner and the syllabus import, which had never been walked -----
  //
  // Reached by **tapping** — Tasks' `Plan` link, then the planner's own import link —
  // because that is the only way a student reaches these screens and therefore the only
  // honest way to walk them. Before those links existed there was no path at all (the two
  // inbound links formed a closed cycle), which is why leg (e) went unmeasured; a surface
  // keyed by URL would have passed while the screens were unreachable.
  //
  // **No `set` and no fixture switch**, deliberately: the import link renders whether or
  // not the account has goals, so one fixture serves all three surfaces. An earlier version
  // emptied `/goals` for the syllabus pair, which does not work here — the loop is
  // theme → scale → surface, `staleTime` is 30 s, and `/goals` is one query key, so the
  // emptied answer was still cached when `planner` ran again. The durable fix was the
  // product one: make the import reachable with goals present.
  {
    name: 'planner',
    expect: 'UTS Kriptografi',
    steps: [{ nav: 'Planner' }, { link: 'Calendar' }],
  },
  {
    name: 'syllabus',
    // **`What to call it`, not `Import a syllabus`** — and the reason is the mistake
    // `study-source`'s guard made. That string is `planner.goals.empty.action` *and*
    // `syllabus.upload.title`, so it renders on the planner too: a click that landed but
    // whose route did not render would still find the text on the page it left, and the
    // surface would measure `/planner` twice while reporting PASS. `nameLabel` is the
    // upload panel's own field label and appears nowhere else in the app.
    expect: 'What to call it',
    steps: [
      { nav: 'Planner' },
      { link: 'Calendar' },
      { link: 'Import another syllabus' },
    ],
  },
  // The review screen a draft opens. **It needs the upload to have happened**, because a
  // draft is listed nowhere — `SyllabusPanel` holds the id in local state from the POST's
  // response — so the steps fill the textarea, read the syllabus, and follow the link that
  // appears. That is the student's own path, and it is why the harness needed a `fill` step.
  {
    name: 'syllabus-review',
    expect: 'Ujian Tengah Semester',
    steps: [
      { nav: 'Planner' },
      { link: 'Calendar' },
      { link: 'Import another syllabus' },
      { fill: '#syllabus-text', value: 'UAS Kriptografi 15 Desember. Tugas 4 minggu depan.' },
      // The upload's button label carries the plan's credit cost, so it is clicked by
      // selector rather than by a label that would have to encode `config/plans.toml`.
      { selector: 'button.btn-primary' },
      // **`syllabus.upload.review`, not the draft's title** — the title is never a link;
      // the link is this label, and it exists only once `draftId !== null`.
      { link: 'Read what was found' },
    ],
  },
  {
    name: 'workspace-sources',
    steps: [{ nav: 'Work' }, { link: WORKSPACE.name }],
  },
  {
    name: 'workspace-chat',
    steps: [{ nav: 'Work' }, { link: WORKSPACE.name }, { tab: 'Chat' }],
  },
  {
    name: 'workspace-studio-flashcards',
    steps: [{ nav: 'Work' }, { link: WORKSPACE.name }, { tab: 'Studio' }],
  },
  // --- The Study hub, the redesign's new destination --------------------------------
  //
  // The hub is the nav's third item and it had never been walked, so nothing measured
  // its two lists, its priority block or its one action. It is reached by its own nav
  // label — the same label at both widths (`Study`) — and the guard is the *deck title*,
  // which comes from the fixture rather than from the copy: an `expect` naming one of the
  // screen's own strings would fail while a translation was being merged, which is a
  // failure about the locale files rather than about the layout.
  //
  // It is also the widest text this walk sees in a list row: the fixture's deck title is
  // 34 characters of Indonesian and its quiz title is longer still, both rendered in a
  // `truncate` beside a chip.
  { name: 'study-hub', expect: 'Kriptografi Kunci Publik', steps: [{ nav: 'Study' }] },
  // --- Phase 4: the study screen, which had never been measured ---------------------
  //
  // Reached the way a phone reaches it — the deck row's own link — because the screen's
  // whole subject is one card and its four grades, and a walk by URL would have passed
  // while the link that opens it was unreachable.
  //
  // **Four surfaces, because the screen has four states that carry different controls.**
  // The unrevealed state has one action; the revealed state has the four grade buttons
  // (the smallest targets on the screen, and the ones gate 5(c) names); the source state
  // adds the passage panel, which is the only place a nested note and a page label meet
  // the card; and the graded-by-key state is the only way to measure that a shortcut
  // *acts* rather than merely exists. One surface can only measure the state it lands in,
  // and the grade row does not exist until the answer is revealed.
  { name: 'study', expect: 'Card 1 of 2', steps: STUDY_STEPS },
  {
    name: 'study-revealed',
    expect: 'Kunci yang boleh diketahui siapa saja',
    words: ['Again', 'Hard', 'Good', 'Easy'],
    steps: [...STUDY_STEPS, { button: 'Show answer' }],
  },
  {
    // `'page 42'` rather than the toggle's own label: `'Hide source'` proves only that the
    // button's text flipped, while `SourcePassage` renders its retry notice instead of the
    // passage when `useChunk` fails — so a 404 on `/chunks/k1` would satisfy a
    // label-based guard and this surface would measure an error notice. The page label
    // exists only when the chunk resolved.
    name: 'study-source',
    expect: 'page 42',
    steps: [...STUDY_STEPS, { button: 'Show answer' }, { button: 'Show source' }],
  },
  // Gate 5(a)'s second half, which no surface above can measure: **`1`-`4` grade
  // without touching the mouse**. Pressing `3` has to *do* something observable, and
  // the observable thing is the advance — the queue moves to card 2 of 2. Asserting
  // only that a key press did not throw would pass on a screen whose handler was
  // never registered, which is the failure this check exists to catch.
  {
    name: 'study-graded-by-key',
    expect: 'Card 2 of 2',
    keys: [{ press: '3' }],
    steps: [...STUDY_STEPS, { button: 'Show answer' }],
  },
  {
    name: 'workspace-studio-quiz',
    steps: [
      { nav: 'Work' },
      { link: WORKSPACE.name },
      { tab: 'Studio' },
      { tab: 'Quiz' },
    ],
  },
  {
    name: 'workspace-studio-report',
    steps: [
      { nav: 'Work' },
      { link: WORKSPACE.name },
      { tab: 'Studio' },
      { tab: 'Report' },
    ],
  },
  // Reached from Settings, the way a phone reaches it, and the one screen on this
  // list with a bar chart in it. Requested by Phase 12's own gate-5 slice, which
  // owns the screen and supplied the fixture bodies below.
  { name: 'dashboard', steps: [{ nav: 'More' }, { link: 'View your progress' }] },
  // The same chat thread with two proposals waiting under it, so the agent cards
  // are measured rather than only their absence. The card renders straight from
  // `GET /actions/pending`, so no step beyond the thread's own is needed — and both
  // tools are in the fixture on purpose, because they stress different things:
  // `create_task` renders the five-field `<dl>` and the longest sentence on the
  // card (`agent.card.noCost`), `create_flashcards` the one that shows a price.
  {
    name: 'workspace-chat-proposal',
    steps: [{ nav: 'Work' }, { link: WORKSPACE.name }, { tab: 'Chat' }],
  },
  // --- Phase 13: the pricing page, and the checkout screen in all six of its states ------
  //
  // Reached the way a phone reaches them, and `/pricing` is the interesting one: it is a
  // **button**, not a link (`BillingScreen`'s "See plans"), so measuring it by URL would
  // pass while it was unreachable by tap — which is the defect the workspace surfaces'
  // comment already names.
  //
  // The six checkout surfaces each `set` the fixture first and then walk the same four
  // steps, because the state is derived from the payment row: one surface can only measure
  // one of them. `offline` is the sixth and the one a fixture body cannot express.
  {
    name: 'pricing',
    expect: 'Plans',
    steps: [
      { nav: 'More' },
      { link: 'Open plan and usage' },
      { button: 'See plans' },
    ],
  },
  {
    name: 'checkout-waiting',
    expect: 'Waiting for confirmation',
    set: () => {
      checkoutStateFixture('waiting')
    },
    words: ['Waiting', 'Waiting for confirmation'],
    steps: CHECKOUT_STEPS,
  },
  {
    name: 'checkout-paid',
    expect: 'Payment received',
    set: () => {
      checkoutStateFixture('paid')
    },
    words: ['Paid', 'Payment received'],
    steps: CHECKOUT_STEPS,
  },
  {
    name: 'checkout-expired',
    expect: 'This code has expired',
    set: () => {
      checkoutStateFixture('expired')
    },
    words: ['Expired', 'This code has expired'],
    steps: CHECKOUT_STEPS,
  },
  {
    name: 'checkout-review',
    expect: 'We are checking this payment',
    set: () => {
      checkoutStateFixture('review')
    },
    words: ['Under review', 'We are checking this payment', 'Do not pay again'],
    steps: CHECKOUT_STEPS,
  },
  {
    name: 'checkout-failed',
    expect: 'The payment did not go through',
    set: () => {
      checkoutStateFixture('failed')
    },
    words: ['Failed', 'The payment did not go through'],
    steps: CHECKOUT_STEPS,
  },
  {
    name: 'checkout-offline',
    expect: 'We could not reach Stvdy',
    set: () => {
      checkoutStateFixture('offline')
    },
    words: ['Not connected', 'We could not reach Stvdy'],
    steps: CHECKOUT_STEPS,
  },
  // --- The admin console, which the redesign added and nothing walked ---------------
  //
  // Reached by **tapping the link Settings renders for an operator**, not by URL, and
  // that is the point of the surface existing: the console is registered for every
  // account and the *server* is the gate (`AdminUser` answers 404 to a non-admin, A7),
  // so before this link there was no path to it at all and a walk by URL would have
  // passed while the screen was unreachable — the same failure Phase 11's planner
  // surfaces were added to close.
  //
  // The guard is a figure from the fixture rather than one of the screen's own labels:
  // a deck title, a course code or a table heading would also be satisfied by the
  // screen's loading state if the copy happened to match, while an account's address is
  // only on the page when the users table resolved.
  { name: 'admin', expect: 'rani.putri@example.test', steps: ADMIN_STEPS },
]

/**
 * The API, from fixtures.
 *
 * Keyed by pathname, because the app's base URL is baked in at build time
 * (`VITE_API_BASE_URL`, defaulting to `http://localhost:8080`) and this script
 * answers that origin whatever it turns out to be.
 */
const ROUTES = {
  // `user` is part of the answer, not decoration: `useSignIn`'s `onSuccess` seeds
  // the session query with it, so a body without it signs the user straight back
  // out and every screen after this one is the sign-in form.
  '/auth/login': () => ({
    access_token: 'access-1',
    refresh_token: 'refresh-1',
    device_id: 'd1',
    user: USER,
  }),
  '/auth/refresh': () => ({ access_token: 'access-1', refresh_token: 'refresh-1', expires_at: null }),
  '/auth/me': () => USER,
  '/usage': () => USAGE,
  // One workspace, so the list renders a row (a name link, a count, Rename and
  // Delete) rather than its empty state — and so the five workspace surfaces below
  // have a link to walk through.
  '/workspaces': () => ({ workspaces: [WORKSPACE] }),
  // The workspace screen's five reads, plus the two per-row status reads the source
  // rows make. A missing one 404s and the walk measures an error notice instead of
  // the screen it meant to measure.
  '/workspaces/w1': () => ({ workspace: WORKSPACE }),
  '/workspaces/w1/sources': () => ({ sources: SOURCES }),
  '/sources/s1/status': () => ({
    id: 's1',
    status: 'ready',
    chunk_count: 48,
    error: null,
    updated_at: '2026-10-11T08:00:00Z',
  }),
  '/sources/s2/status': () => ({
    id: 's2',
    status: 'ready',
    chunk_count: 21,
    error: null,
    updated_at: '2026-10-11T08:00:00Z',
  }),
  '/workspaces/w1/messages': () => ({ messages: MESSAGES }),
  // Two pending proposals rather than none: the card is what the chat surface has
  // to fit a five-field definition list into, and an empty list measures the
  // screen without it.
  '/actions/pending': () => ({ actions: PENDING_ACTIONS }),
  '/workspaces/w1/decks': () => ({
    decks: [DECK],
    cost: { per_batch: 4, batch_size: 10, max_cards: 30 },
  }),
  // The study screen's three reads: the deck (its title and, for the header, the
  // workspace the "Back to the workspace" link points at), the due queue (which
  // carries the server's `now`, the instant the client filters and orders with),
  // and the passage a revealed card's source panel shows. Without the last one the
  // panel 404s and the walk measures an error notice where the passage belongs.
  '/decks/d1': () => ({ deck: DECK, cards: CARDS }),
  '/decks/d1/due': () => ({ cards: CARDS, now: '2026-10-12T02:00:00Z' }),
  '/chunks/k1': () => CHUNK,
  // The review the number keys post. Without it the mutation 404s, `StudyScreen`
  // keeps the card and shows its retry notice, and a surface that presses `3` would
  // measure a failed review rather than the advance a grade produces.
  '/flashcards/c1/review': () => ({
    applied: true,
    card: { id: 'c1', ease: 2.5, interval_days: 1, due_at: '2026-10-13T02:00:00Z' },
  }),
  '/workspaces/w1/quizzes': () => ({
    quizzes: [QUIZ],
    cost: { per_batch: 6, batch_size: 10, max_questions: 20 },
  }),
  '/workspaces/w1/reports': () => ({ reports: [REPORT] }),
  // The two Phase 12 screens' reads. `/dashboard` is requested with a query string
  // and the table is keyed by pathname, so one entry answers every window.
  '/dashboard': () => ({
    from: '2026-09-12',
    to: '2026-10-12',
    time_per_course: [
      { workspace_id: 'w1', title: WORKSPACE.name, minutes: 415 },
      { workspace_id: 'w2', title: 'Aljabar Linier dan Matriks', minutes: 95 },
    ],
    review_activity: [
      { local_date: '2026-10-06', reviews: 34 },
      { local_date: '2026-10-07', reviews: 0 },
      { local_date: '2026-10-08', reviews: 61 },
    ],
    totals: { minutes_studied: 510, reviews: 95, cards_due: 27, quizzes_taken: 6 },
  }),
  '/dashboard/topics': () => ({
    quiz_mastery: [
      {
        source_id: 's1',
        title: 'Fungsi Hash dan Integritas Pesan',
        attempts: 24,
        correct: 19,
        accuracy: 0.79,
      },
      {
        source_id: 's2',
        title: 'Algoritma Kunci Publik',
        attempts: 8,
        correct: 3,
        accuracy: 0.375,
      },
    ],
    weak_topics: [
      { source_id: 's2', title: 'Algoritma Kunci Publik', attempts: 8, correct: 3, accuracy: 0.375 },
    ],
  }),
  '/tasks': () => ({ tasks: [TASK] }),
  '/settings': () => ({ settings: SETTINGS_ROW, timezone: USER.timezone }),
  // The calendar reads these five; a missing one 404s and the walk measures the
  // failure state instead of the screen.
  '/events': () => ({ events: [EVENT] }),
  '/calendar/connections': () => ({ connections: [CONNECTION] }),
  '/calendar/ics-token': () => ({ ics_feed_token: 'feed-token-1' }),
  '/calendar/google/connect': () => ({ url: 'https://accounts.google.com/o/oauth2/v2/auth' }),
  '/sessions': () => ({ sessions: [] }),
  '/devices': () => ({ devices: [] }),
  // Phase 13's three reads. `/plans` is also what the existing `plan` surface needed:
  // `/billing` renders `BillingScreen`, and without this entry the walk measured that
  // panel's 404 notice instead of its content.
  '/plans': () => PLANS_VIEW,
  // Phase 11's three screens. `/goals` and `/goals/g1/progress` are both required for
  // `/planner`: the screen auto-selects its first goal (`rows[0]`), so the progress card
  // renders without a click and a missing progress fixture would measure its error notice.
  '/goals': () => ({ goals: [GOAL] }),
  '/goals/g1/progress': () => GOAL_PROGRESS,
  '/syllabus/d1/draft': () => SYLLABUS_DRAFT,
  // The upload itself, so the review link can appear. `SyllabusPanel` holds the returned
  // id in local state, so this is what makes `draftId !== null` true.
  '/workspaces/w1/syllabus': () => ({ draft_id: 'd1', source_id: 's1' }),
  '/billing/invoices': () => INVOICES_VIEW,
  // The checkout screen's two calls. The poll is keyed by the id the checkout returns, and
  // `CHECKOUT` is what the six state surfaces switch between walks — so every
  // `/billing/payments/<id>` answers from the fixture rather than from a table keyed by one
  // id. `PAYMENT_ID` is kept for the fixture's own default.
  '/billing/checkout': () => paymentRow(),
  [`/billing/payments/${PAYMENT_ID}`]: () => paymentRow(),
  // The admin console's three reads. Every figure is a row count or a sum, and the two
  // tripwire values are deliberately one of each kind: `plan_loss_two_months_running` is
  // `null` (the range holds one month, so the question cannot be answered) while
  // `free_spend_over_quarter_of_revenue` is `true`. A walk that measured only the `false`
  // rows would not see the three-valued rendering the screen exists to get right.
  '/admin/users': () => ({
    from: '2026-09-12T00:00:00Z',
    to: '2026-10-12T00:00:00Z',
    users: [
      {
        id: 'u1',
        email: 'rani.putri@example.test',
        plan_id: 'pro',
        created_at: '2026-09-01T08:00:00Z',
        requests: 42,
        sources_added: 3,
        last_active_at: '2026-10-12T09:30:00Z',
        is_admin: true,
      },
      {
        id: 'u2',
        email: 'budi@example.test',
        plan_id: 'free',
        created_at: '2026-10-01T08:00:00Z',
        requests: 0,
        sources_added: 0,
        last_active_at: null,
        is_admin: false,
      },
    ],
  }),
  '/admin/payments': () => ({
    count: 1,
    payments: [
      {
        id: 'p1',
        user_id: 'u1',
        kind: 'subscription',
        plan_id: 'pro',
        billing_period: 'monthly',
        credits: null,
        provider: 'qris',
        order_id: 'ORD-2026-0001',
        amount_idr: 45000,
        gateway_fee_idr: 3150,
        status: 'review',
        review_reason: 'amount 44000 does not match 45000',
        expires_at: '2026-10-12T05:50:00Z',
        paid_at: null,
        created_at: '2026-10-12T05:35:00Z',
      },
    ],
  }),
  '/admin/margin': () => ({
    from: '2026-09-12T00:00:00Z',
    to: '2026-10-12T00:00:00Z',
    idr_per_usd: 16000,
    months: [
      {
        month: '2026-10',
        per_plan: [
          {
            plan_id: 'pro',
            provider_cost_usd: 0.5625,
            byok_cost_usd: 0,
            revenue_idr: 45000,
            gross_margin_idr: 36000,
            ceiling_usd: 10,
          },
        ],
        revenue_idr: 45000,
        gateway_fee_idr: 3150,
        free_utilization: 0.28,
        paying: 3,
        free_accounts: 4,
        conversion_of_free: 0.75,
        conversion_of_total: 0.43,
        as_of: '2026-10-12T00:00:00Z',
      },
    ],
    tripwires: {
      free_utilization_over_30: false,
      conversion_under_3: false,
      free_spend_over_quarter_of_revenue: true,
      plan_loss_two_months_running: null,
    },
  }),
}

/**
 * The route table, plus the one dynamic path it needs.
 *
 * `/billing/payments/<id>` cannot be a key in `ROUTES`, because the id is what the state
 * fixture switches on and the table is built once. Looking it up here keeps the static map
 * as the one place a fixture is written, and it is deliberately a prefix match on the
 * documented route rather than a wildcard.
 */
function routeFor(pathname) {
  const handler = ROUTES[pathname]
  if (handler !== undefined) return handler
  if (/^\/billing\/payments\/[A-Za-z0-9-]+$/.test(pathname)) return () => paymentRow()
  return undefined
}

/** Fails the run with a readable reason rather than a stack trace. */
function fail(message) {
  console.error(`FAIL: ${message}`)
  process.exit(1)
}

/**
 * What the page measures, injected into the tab.
 *
 * One function for every number the gate reports, so a target measured here and a
 * ratio measured here are the same arithmetic — the reasoning Phase 9's gate
 * harness states, and the reason this is a single injected function rather than a
 * handful of `page.evaluate` snippets.
 *
 * Contrast is computed on **effective** colours, not on the declared ones: the
 * background a reader actually sees behind an element is its own background
 * composited over every translucent ancestor down to the page's. A chip inside a
 * card inside the shell has three backgrounds stacked, and reading only the
 * nearest one reports the ratio against a colour nobody sees.
 */
function measureInPage(options) {
  const { minTarget, minText, minLarge, desktopFloor } = options

  const RGBA = /rgba?\(([^)]+)\)/
  const parseColor = (value) => {
    const match = String(value ?? '').match(RGBA)
    if (!match) return null
    const parts = match[1].split(',').map((part) => parseFloat(part.trim()))
    return { r: parts[0], g: parts[1], b: parts[2], a: parts.length > 3 ? parts[3] : 1 }
  }
  const linear = (channel) => {
    const v = channel / 255
    return v <= 0.03928 ? v / 12.92 : Math.pow((v + 0.055) / 1.055, 2.4)
  }
  const luminance = (c) => 0.2126 * linear(c.r) + 0.7152 * linear(c.g) + 0.0722 * linear(c.b)
  const contrast = (fg, bg) => {
    const a = luminance(fg)
    const b = luminance(bg)
    return Math.round(((Math.max(a, b) + 0.05) / (Math.min(a, b) + 0.05)) * 100) / 100
  }
  const pageBackground = () => {
    for (const node of [document.documentElement, document.body]) {
      const c = parseColor(getComputedStyle(node).backgroundColor)
      if (c && c.a === 1) return c
    }
    return { r: 255, g: 255, b: 255, a: 1 }
  }
  const effectiveBackground = (el) => {
    const stack = []
    let node = el
    while (node && node.nodeType === 1) {
      const c = parseColor(getComputedStyle(node).backgroundColor)
      if (c && c.a > 0) {
        stack.push(c)
        if (c.a === 1) break
      }
      node = node.parentElement
    }
    let base = pageBackground()
    for (let i = stack.length - 1; i >= 0; i--) {
      const c = stack[i]
      base = {
        r: c.r * c.a + base.r * (1 - c.a),
        g: c.g * c.a + base.g * (1 - c.a),
        b: c.b * c.a + base.b * (1 - c.a),
        a: 1,
      }
    }
    return base
  }
  /** A short, readable name for an element, for the report. */
  const describe = (el) => {
    const id = el.id === '' ? '' : `#${el.id}`
    const classes = [...el.classList].slice(0, 3).join('.')
    return `${el.tagName.toLowerCase()}${id}${classes === '' ? '' : `.${classes}`}`
  }
  const rect = (el) => {
    const box = el.getBoundingClientRect()
    return {
      w: Math.round(box.width * 10) / 10,
      h: Math.round(box.height * 10) / 10,
      top: Math.round(box.top * 10) / 10,
      left: Math.round(box.left * 10) / 10,
    }
  }
  /** Visible to a reader: not `display: none`, not `visibility: hidden`, not 0x0. */
  const isVisible = (el) => {
    const style = getComputedStyle(el)
    if (style.display === 'none' || style.visibility === 'hidden') return false
    if (Number(style.opacity) === 0) return false
    const box = el.getBoundingClientRect()
    return box.width > 0 && box.height > 0
  }

  // --- 1. interactive targets -------------------------------------------------
  //
  // The selectors are the plan's, plus the two role-shaped controls the app uses
  // instead of native elements. A checkbox's own box is 13px; its `<label>` is the
  // hit area and is what a finger reaches, so a target is measured as the union of
  // itself and the label that points at it. Measuring the input alone would report
  // a false failure on every checkbox in the app, and a gate that cries wolf gets
  // deleted.
  const TARGETS = 'a[href],button,input,select,textarea,[role="button"],[role="tab"]'
  const hitArea = (el) => {
    if (el.tagName === 'INPUT' && (el.type === 'checkbox' || el.type === 'radio')) {
      const label = el.id === '' ? null : document.querySelector(`label[for="${CSS.escape(el.id)}"]`)
      const owner = label ?? el.closest('label')
      if (owner !== null) return owner
    }
    return el
  }
  const targets = []
  // The keyboard leg walks **these exact elements** rather than re-deriving "a
  // control" from `TARGETS`: a second definition of the set would measure a different
  // set the first time either side changed, and the two would then disagree about
  // what a control is. Stashed on `window` rather than returned, because the walk
  // needs real key presses *between* reads and `page.evaluate` cannot hand element
  // references back to Node.
  const controlElements = []
  for (const el of document.querySelectorAll(TARGETS)) {
    if (!isVisible(el)) continue
    // A skip link is `sr-only` until focused: it is 1x1 by design, and what makes
    // it usable is that focusing it expands it. Measured focused, below.
    if (el.classList.contains('sr-only') && document.activeElement !== el) continue
    // **A citation chip inside a sentence is not a pointer target, and SC 2.5.8 says
    // so.** `button.cite` is 72.8x30.4 at 200% text and 36.4x15.2 at 100% -- at 100%
    // its height is the line-height of the prose it sits in (15.2 against 15.225), so
    // it is a mark between two words rather than a control beside them. The success
    // criterion's own exception list covers a target "in a sentence or block of
    // text", and the 8px-gap clause cannot be satisfied by something set inside a
    // line. Measuring it here would make the desktop leg permanently red on a
    // deliberate decision, which is the failure mode this exemption exists to stop:
    // the count would be ignored rather than read. **The phone rule is untouched** --
    // `button.cite` carries a 44px box inside `@media (max-width: 767px)` for exactly
    // this reason, so at phone width it measures as a target and passes.
    //
    // Recorded in A323 (Phase 14's gate-5 decision) and beside `.cite` in
    // `app/src/styles.css`, so the next phase finds the reason where the rule is.
    if (el.classList.contains('cite') && desktopFloor === true) continue
    controlElements.push(el)
    const box = rect(hitArea(el))
    const ok = box.w >= minTarget && box.h >= minTarget
    targets.push({
      el: describe(el),
      text: (el.textContent ?? el.getAttribute('aria-label') ?? '').trim().slice(0, 40),
      ...box,
      ok,
    })
  }
  const undersized = targets.filter((target) => !target.ok)

  // --- 2. contrast ------------------------------------------------------------
  //
  // Every element with its own text, so the pair measured is the one a reader sees.
  // An element with no text of its own (a wrapper `<div>`) is skipped: it renders
  // no glyph, and its computed `color` belongs to no one.
  const contrasts = []
  const seen = new Set()
  for (const el of document.querySelectorAll('body *')) {
    if (!isVisible(el)) continue
    const own = [...el.childNodes]
      .filter((node) => node.nodeType === 3)
      .map((node) => node.textContent.trim())
      .join('')
      .trim()
    if (own === '') continue
    const style = getComputedStyle(el)
    const fg = parseColor(style.color)
    if (fg === null) continue
    const bg = effectiveBackground(el)
    const px = parseFloat(style.fontSize)
    const weight = Number(style.fontWeight) || 400
    // WCAG's large-text definition: >=24px, or >=18.66px when bold.
    const large = px >= 24 || (px >= 18.66 && weight >= 700)
    const ratio = contrast(fg, bg)
    const need = large ? minLarge : minText
    const key = `${describe(el)}|${own.slice(0, 24)}|${px}|${need}`
    if (seen.has(key)) continue
    seen.add(key)
    contrasts.push({
      el: describe(el),
      text: own.slice(0, 40),
      px,
      color: style.color,
      bg: `rgb(${bg.r}, ${bg.g}, ${bg.b})`,
      ratio,
      need,
      pass: ratio >= need,
    })
  }
  const below = contrasts.filter((pair) => !pair.pass)

  // --- 3. no clipping, no overlap ---------------------------------------------
  //
  // (a) The page must not scroll sideways. A horizontal scrollbar on a phone means
  // something is wider than the viewport, which is how Phase 6 found the tab bar's
  // fifth item painted 45px past the right edge.
  const scrolling = document.scrollingElement
  const pageOverflow = {
    scrollWidth: scrolling.scrollWidth,
    clientWidth: scrolling.clientWidth,
    overflows: scrolling.scrollWidth > scrolling.clientWidth,
  }

  // (b) An element whose own content is wider or taller than its box, with no
  // scroll container of its own, is content a reader cannot reach. `overflow-x-auto`
  // is the app's sanctioned answer (the tab bar's own fix), so a scrollable element
  // is not a failure — it is the fix working.
  //
  // Three exclusions, each of which would otherwise report the correct build:
  // `.sr-only` is 1x1 by design (that is what makes it screen-reader-only), a form
  // control clips its own value by design and reports it as scroll overflow, and an
  // *ancestor* of an offending element reports the same overflow again — so only the
  // innermost element of a chain is reported, which is the one that is actually too
  // wide.
  //
  // A fourth: `truncate` is `overflow: hidden; text-overflow: ellipsis;
  // white-space: nowrap`, so an element carrying it *declares* that its text will be
  // cut off and shows an ellipsis to say so — it is the sanctioned single-line label
  // treatment, and reporting it as such would fail the gate on the pattern rather
  // than on a defect.
  //
  // It is excluded **only when the element also carries a non-empty `title`**, and
  // that condition is the whole point rather than a detail. An ellipsis on a phone
  // has no hover, so a truncated string with nothing else carrying it is text a
  // reader cannot reach — which is the same defect this check exists for, arriving
  // by a different route. With a `title` the full string is recoverable (by long
  // press, by the accessibility tree, by a desktop hover), so the truncation is a
  // presentation choice; without one it is a dead end and stays a failure.
  const clipped = []
  const CLIPS_BY_DESIGN = new Set(['INPUT', 'SELECT', 'TEXTAREA'])
  for (const el of document.querySelectorAll('body *')) {
    if (!isVisible(el)) continue
    if (el.classList.contains('sr-only')) continue
    if (el.classList.contains('truncate') && (el.getAttribute('title') ?? '').trim() !== '') {
      continue
    }
    if (CLIPS_BY_DESIGN.has(el.tagName)) continue
    const style = getComputedStyle(el)
    if (style.overflowX === 'auto' || style.overflowX === 'scroll') continue
    if (style.overflowY === 'auto' || style.overflowY === 'scroll') continue
    // 1px of slack: sub-pixel layout rounding is not a defect.
    const wide = el.scrollWidth > el.clientWidth + 1
    const tall = el.scrollHeight > el.clientHeight + 1
    if (!wide && !tall) continue
    clipped.push({
      node: el,
      el: describe(el),
      text: (el.textContent ?? '').trim().slice(0, 40),
      scrollWidth: el.scrollWidth,
      clientWidth: el.clientWidth,
      scrollHeight: el.scrollHeight,
      clientHeight: el.clientHeight,
    })
  }
  const innermost = clipped.filter(
    (entry) => !clipped.some((other) => other !== entry && entry.node.contains(other.node)),
  )
  const clippedReport = innermost.map(({ node: _node, ...rest }) => rest)

  // (c) Overlap: a control that is painted over by something else, **at a scroll
  // position where it should be reachable**.
  //
  // This is the check that caught Phase 8's band: the tab bar was `fixed` over the
  // content, so the last line of a paragraph sat behind it and `elementFromPoint`
  // at that point returned the chip. It has to be scroll-aware, because a sticky
  // band legitimately covers content *while scrolling* — that is what sticky is
  // for. What is a defect is a control that stays covered after it is scrolled into
  // the middle of the viewport, which no scroll position can fix.
  //
  // The walk moves the page and every scroll container, so both are recorded and
  // restored: leaving the tab bar scrolled would make the next check report its
  // items as painted past the left edge, which is the fix for Phase 6's defect
  // working rather than a defect.
  const overlaps = []
  const restore = { x: window.scrollX, y: window.scrollY }
  const scrollers = [...document.querySelectorAll('body *')]
    .filter((el) => el.scrollWidth > el.clientWidth || el.scrollHeight > el.clientHeight)
    .map((el) => ({ el, left: el.scrollLeft, top: el.scrollTop }))
  for (const el of document.querySelectorAll(TARGETS)) {
    if (!isVisible(el)) continue
    const owner = hitArea(el)
    owner.scrollIntoView({ block: 'center' })
    const box = owner.getBoundingClientRect()
    const x = box.left + box.width / 2
    const y = box.top + box.height / 2
    if (x < 0 || y < 0 || x > window.innerWidth || y > window.innerHeight) continue
    const top = document.elementFromPoint(x, y)
    if (top === null) continue
    if (top === owner || owner.contains(top) || top.contains(owner)) continue
    overlaps.push({
      el: describe(el),
      text: (el.textContent ?? el.getAttribute('aria-label') ?? '').trim().slice(0, 40),
      coveredBy: describe(top),
      at: `${Math.round(x)},${Math.round(y)}`,
    })
  }
  for (const scroller of scrollers) {
    scroller.el.scrollLeft = scroller.left
    scroller.el.scrollTop = scroller.top
  }
  window.scrollTo(restore.x, restore.y)

  // (d) Anything painted outside the viewport's width.
  //
  // This is a different question from (b) and it is the one that names the culprit:
  // an element wider than its parent is simply wide — its own `scrollWidth` equals
  // its `clientWidth` — while the *parent* reports the overflow. So (b) says "the
  // shell is too wide" and this says which box made it so, which is what Phase 6's
  // gate needed when the tab bar's fifth item was painted 45px past the right edge
  // with nothing to scroll it into view.
  //
  // An element inside a horizontal scroll container is excluded, and that exclusion
  // is the whole reason the tab bar's fifth item is *not* a defect now: the bar
  // scrolls, so its items are reachable by swipe and by `Tab` even though four of
  // the five sit outside the viewport at 200% type. Reporting them would fail the
  // gate on its own fix.
  const inScroller = (el) => {
    for (let node = el.parentElement; node !== null; node = node.parentElement) {
      const style = getComputedStyle(node)
      if (style.overflowX === 'auto' || style.overflowX === 'scroll') return true
    }
    return false
  }
  const pastEdge = []
  for (const el of document.querySelectorAll('body *')) {
    if (!isVisible(el)) continue
    if (el.classList.contains('sr-only')) continue
    if (inScroller(el)) continue
    const box = el.getBoundingClientRect()
    const right = box.right - window.innerWidth
    const left = -box.left
    if (right <= 1 && left <= 1) continue
    pastEdge.push({
      node: el,
      el: describe(el),
      text: (el.textContent ?? '').trim().slice(0, 40),
      width: Math.round(box.width * 10) / 10,
      past: Math.round(Math.max(right, left) * 10) / 10,
      side: right > left ? 'right' : 'left',
    })
  }
  const pastEdgeReport = pastEdge
    .filter((entry) => !pastEdge.some((other) => other !== entry && entry.node.contains(other.node)))
    .map(({ node: _node, ...rest }) => rest)

  // --- 4. the keyboard walk's page-side half ----------------------------------
  //
  // Gate 5(a) — "reach every control by keyboard alone with a visible focus ring" —
  // is the one criterion this harness never measured, so it was done by hand
  // (`var/gate5-keyboard.json`, three report screens) and was not reproducible from
  // any committed command. It cannot be measured from one `page.evaluate` either: a
  // real key press has to happen *between* two reads of `document.activeElement`,
  // and an element reference cannot cross back to Node. So the walk is driven from
  // Node and this object is its page-side half — the **same** `controlElements` the
  // target check just built rather than a second definition of "a control" (two
  // definitions would disagree about the set within a month), the same `describe`,
  // and one read per press.
  //
  // **What the ring assertion is, and why it is not stricter.** A focused control
  // must show *some* indicator: a non-zero outline, or a non-`none` box-shadow (a
  // `ring` utility is a shadow). It deliberately asserts no colour, width or ratio:
  // `--ring` is theme-keyed (`#4f46e5` on light, `#a5b4fc` on dark), so a check
  // pinned to one of them is a check that breaks the first time a theme is added or
  // tuned — and a gate that breaks on a deliberate change is a gate that gets
  // deleted rather than fixed. The ring's contrast against its own background is the
  // contrast check's business, not this one's.
  //
  // The `sr-only` skip link is deliberately **not** in the expected set (the target
  // check excludes it: it is 1x1 until focused), but it *is* the page's first focus
  // stop, so it is reached, and ring-checked, like any other stop.
  //
  // **What "reached" means, and the three classes it does not apply to.** The
  // assertion is "every control the platform puts in the tab order is reached by
  // Tab". Three kinds of control are *not* in the tab order, by the platform's rule
  // or by a sanctioned ARIA pattern, and each is excused with its reason rather than
  // reported — the same judgement the `cite` exemption below makes, and for the same
  // reason: a gate that is permanently red on a deliberate decision is a gate that
  // gets ignored rather than read.
  //
  //   1. **`disabled`** (including a control inside a disabled `<fieldset>`, which is
  //      why this asks `:disabled` and not `.disabled` — the property reads the
  //      attribute, so a fieldset's children would look enabled). Not operable by any
  //      input, and the browser itself removes it from the tab order: asserting Tab
  //      must reach it asserts something the platform forbids. The fixture's forms
  //      are empty, so their submits are disabled in the resting state the walk
  //      measures — a reader who types into the form gets them back.
  //   2. **A radio group is one tab stop.** Only the checked member is in the tab
  //      order; the platform moves between the others with the arrow keys. The group
  //      still has to be reachable, so a group with *no* member reached by Tab is a
  //      finding — the exemption is conditional on that.
  //   3. **A roving-tabindex `[role="tab"]` tablist is one tab stop**, the WAI-ARIA
  //      tabs pattern: `tabIndex={selected ? 0 : -1}` in `WorkspaceScreen.tsx`, whose
  //      `onKeyDown` moves with ArrowLeft/ArrowRight/Home/End. Same conditional
  //      exemption as (2), and the arrow keys are *measured* rather than assumed —
  //      `arrowGroups` below hands the walk the groups it has to prove.
  //
  // Excused controls are returned with their reason, so the count is printed and the
  // elements are in the JSON: an exemption that is silent is the quiet sweep this leg
  // exists to avoid.
  const notOperable = (el) =>
    el.matches(':disabled') || el.getAttribute('aria-disabled') === 'true'
  /**
   * The group a control shares its single tab stop with, or null.
   *
   * **Membership is by the group, not by the member's own exclusion**, and the first
   * run of this leg is why that distinction is written down: grouping only the
   * `tabindex="-1"` tabs left every tablist's `reached` count at zero — the one tab that
   * *is* a Tab stop is the active one, `tabIndex={selected ? 0 : -1}` — so the phone
   * walk's 36 tab strips (9 per combination) were reported as 72 unreached controls. The
   * group is what has to be reachable;
   * each member is then excused by the group's own count.
   */
  const groupOf = (el) => {
    if (notOperable(el)) return null
    if (el.tagName === 'INPUT' && el.type === 'radio') {
      const form = el.form === null ? 'none' : String(idOf(el.form))
      return { key: `radio:${form}:${el.name}`, reason: 'radio' }
    }
    if (el.getAttribute('role') === 'tab') {
      // No tablist around it: the element is its own group, so it can only be excused by
      // being reached itself — a lone `role="tab"` with `tabindex="-1"` is a finding.
      const list = el.closest('[role="tablist"]')
      return { key: `tab:${String(idOf(list ?? el))}`, reason: 'roving-tab' }
    }
    return null
  }
  const stops = new Map()
  const stopOrder = []
  const ids = new Map()
  let nextId = 0
  const idOf = (el) => {
    const found = ids.get(el)
    if (found !== undefined) return found
    const id = nextId
    nextId += 1
    ids.set(el, id)
    return id
  }
  const labelOf = (el) =>
    (el.textContent ?? el.getAttribute('aria-label') ?? '').trim().slice(0, 40)
  /** Every group of controls that share one tab stop, with how many of them Tab reached. */
  const groupState = () => {
    const groups = new Map()
    for (const el of controlElements) {
      const found = groupOf(el)
      if (found === null) continue
      const group =
        groups.get(found.key) ?? { key: found.key, reason: found.reason, members: [], reached: 0 }
      group.members.push(el)
      if (stops.has(el)) group.reached += 1
      groups.set(found.key, group)
    }
    return groups
  }
  let walkScroll = null
  window.__gate5Keyboard = {
    /** How many controls the walk has to reach: the target check's own set. */
    expected: controlElements.length,
    /** Whatever holds focus right now, with the focus indicator it is showing. */
    read: () => {
      const el = document.activeElement
      // `body` and `documentElement` are what Chrome reports when focus has left the
      // page for its own chrome; they are not a focus stop a reader made.
      if (!(el instanceof HTMLElement) || el === document.body || el === document.documentElement) {
        return { id: -1, none: true }
      }
      const style = getComputedStyle(el)
      const width = Number.parseFloat(style.outlineWidth)
      const hasOutline = Number.isFinite(width) && width > 0 && style.outlineStyle !== 'none'
      const shadow = style.boxShadow
      const hasShadow = shadow !== 'none' && shadow !== ''
      const id = idOf(el)
      if (!stops.has(el)) {
        stops.set(el, id)
        stopOrder.push({ id, el: describe(el), text: labelOf(el) })
      }
      return {
        id,
        none: false,
        el: describe(el),
        text: labelOf(el),
        outline: `${style.outlineStyle} ${style.outlineWidth}`,
        shadow,
        ringOk: hasOutline || hasShadow,
        // A composite date/time input is one element to `document.activeElement` but
        // several stops to Tab: the platform moves between its internal month/day/year
        // fields (and its picker button) while the host keeps focus, so a repeated read
        // of it is the platform working, not a trap. Flagged here because only the page
        // knows what the element is.
        composite:
          el.tagName === 'INPUT' &&
          ['date', 'time', 'datetime-local', 'month', 'week'].includes(el.type),
      }
    },
    /**
     * Puts the sequential focus navigation start point back at the top of the document,
     * and reports what held focus before it did.
     *
     * **This is what makes the walk cover the whole focus order rather than its tail,
     * and the first run proved it.** Chrome keeps the start point across a navigation:
     * on the `today` surface — the first one after `signIn`, whose DOM swap removed the
     * focused submit button — the first `Tab` went straight to the browser chrome and
     * the walk measured **0 stops for 9 controls**, while every later surface (whose
     * start point was a `blur()` inside the page) began at the skip link. A walk that
     * starts mid-document measures a suffix and reports it as the whole: focusing the
     * body through a temporary `tabindex="-1"` (removed again before the first press, so
     * the body is not itself a stop) puts the start point where a reader arriving on the
     * page has it.
     */
    resetStart: () => {
      const before = document.activeElement
      const at =
        before instanceof HTMLElement && before !== document.body
          ? describe(before)
          : 'nothing in the page'
      document.body.setAttribute('tabindex', '-1')
      document.body.focus()
      document.body.removeAttribute('tabindex')
      return at
    },
    /** Where the page and its scrollers were before the walk, so they can be put back. */
    captureScroll: () => {
      walkScroll = {
        x: window.scrollX,
        y: window.scrollY,
        scrollers: [...document.querySelectorAll('body *')]
          .filter((el) => el.scrollWidth > el.clientWidth || el.scrollHeight > el.clientHeight)
          .map((el) => ({ el, left: el.scrollLeft, top: el.scrollTop })),
      }
    },
    /**
     * Puts the page back the way the walk found it.
     *
     * Tabbing scrolls each focused element into view — including inside a scroller, and
     * the tab bar *is* one — so every scroller goes back to the offset it had. **The
     * blur is load-bearing, not tidiness:** `measureInPage` includes an `sr-only`
     * control only while it holds focus (that is how the skip link is measured as the
     * 1x1 target it is at rest), so a walk that ended on the skip link would make the
     * *next* surface report it as an undersized target — the keyboard leg changing the
     * numbers of the checks that already passed.
     */
    restore: () => {
      for (const scroller of walkScroll?.scrollers ?? []) {
        scroller.el.scrollLeft = scroller.left
        scroller.el.scrollTop = scroller.top
      }
      if (walkScroll !== null) window.scrollTo(walkScroll.x, walkScroll.y)
      const el = document.activeElement
      if (el instanceof HTMLElement) el.blur()
    },
    /**
     * The reach assertion, which needs element identity and so can only be answered here.
     *
     * `unreached` is what the leg reports as a problem: a control the platform keeps in
     * the tab order that no press reached. `excused` is the three classes above with
     * their reason and their element, so the exemption is visible rather than silent.
     * `groups` is the subset of them the walk still has to *prove* with arrow keys —
     * a group with at least one member reached by Tab and at least one that is not.
     */
    summary: () => {
      const groups = groupState()
      const unreached = []
      const excused = []
      for (const el of controlElements) {
        if (stops.has(el)) continue
        if (notOperable(el)) {
          excused.push({ el: describe(el), text: labelOf(el), reason: 'disabled' })
          continue
        }
        const found = groupOf(el)
        const group = found === null ? undefined : groups.get(found.key)
        if (group !== undefined && group.reached > 0) {
          excused.push({ el: describe(el), text: labelOf(el), reason: group.reason })
          continue
        }
        unreached.push({ el: describe(el), text: labelOf(el) })
      }
      return {
        focusStops: stopOrder.length,
        unreached,
        excused,
        groups: [...groups.values()]
          .filter((group) => group.reached > 0 && group.reached < group.members.length)
          .map((group) => ({
            reason: group.reason,
            members: group.members.length,
            reachedByTab: group.reached,
            // The member to put focus on before pressing an arrow key: one that Tab
            // already reached, so the probe starts from where the pattern says a reader
            // is. `focusStop` turns this back into an element.
            focus: idOf(group.members.find((el) => stops.has(el)) ?? group.members[0]),
            ids: group.members.map((el) => idOf(el)),
          })),
      }
    },
    /** Focuses a stop by its id, for the arrow-key probe. Returns whether it landed. */
    focusStop: (id) => {
      const el = [...ids.entries()].find(([, value]) => value === id)?.[0]
      if (el === undefined || !(el instanceof HTMLElement)) return false
      el.focus()
      return document.activeElement === el
    },
    /** The focused stop's id, for the arrow-key probe. Records no stop. */
    focused: () => {
      const el = document.activeElement
      if (!(el instanceof HTMLElement) || el === document.body) return -1
      return ids.get(el) ?? -1
    },
    /** Element detail for every id this surface handed out, so Node can name what it found. */
    info: () =>
      [...ids.entries()].map(([el, id]) => ({ id, el: describe(el), text: labelOf(el) })),
  }

  return {
    targets,
    undersized,
    contrasts,
    below,
    pageOverflow,
    clipped: clippedReport,
    overlaps,
    pastEdge: pastEdgeReport,
    scale: getComputedStyle(document.documentElement).fontSize,
  }
}

/** Signs in through the app's own form, so the shell and the session are real. */
async function signIn(page) {
  await page.waitForSelector('input[type="email"]', { timeout: 30000 })
  await page.type('input[type="email"]', 'student@example.com')
  await page.type('input[type="password"]', 'correct horse battery')
  await page.click('button[type="submit"]')
  await page.waitForFunction(
    () => [...document.querySelectorAll('a')].some((a) => a.textContent.trim() === 'Today'),
    { timeout: 30000 },
  )
}

/**
 * Navigates by clicking, not by changing the URL.
 *
 * The access token lives in memory only on the web build, so a reload would drop
 * the session and every measurement after the first surface would be of a sign-in
 * screen. Clicking is also what a phone does, and it exercises the tab bar itself.
 *
 * Four kinds of step, because a phone reaches a screen four ways: a `nav` is one
 * of the five tab-bar destinations, a `link` is any `<a>` the screen renders, a
 * `tab` is a `role="tab"` button in one of the two panel strips, and a `button` is
 * any other `<button>`. The last one was added by Phase 13 and it is not
 * decoration either: `/pricing` and the checkout screen are reached through
 * **buttons** (`BillingScreen`'s "See plans" and a plan card's "Choose Pro"), so a
 * walk that could only follow links would have had to measure those two screens by
 * URL — which passes while the screen is unreachable by tap.
 *
 * A `set` step is different in kind: it runs here, in Node, and changes a fixture
 * for the surface about to be walked. The checkout screen's state is derived from
 * the payment row, so reaching `paid`/`expired`/`review`/`failed`/`offline` means
 * answering the next poll differently — one surface can only measure one of them.
 *
 * `keys` is the one thing the Tab walk cannot check: a shortcut that has to *act*
 * rather than merely be reachable. The keys are pressed after the steps and the
 * surface's `expect` is asserted after them, so a key with no handler leaves the
 * screen where the steps left it and the surface fails.
 */
async function openSurface(page, surface) {
  // A fixture switch, not a tap: it changes what the *next* request is answered with, so
  // it runs before the first step touches the page.
  if (typeof surface.set === 'function') surface.set()
  for (const step of surface.steps) {
    // A `fill` step writes a value into a field, which the four tap kinds cannot express.
    // The syllabus review screen is why it exists: a draft is only reachable after an
    // upload, and the upload needs text in `#syllabus-text` — so without this the screen
    // is reachable only by URL, which is the thing the walk refuses to do.
    //
    // The value is set through the native setter and an `input` event, because React
    // tracks the last value it wrote and ignores a plain `element.value = …`.
    if (step.fill !== undefined) {
      const written = await page.evaluate(
        (selector, value) => {
          const field = document.querySelector(selector)
          if (field === null) return false
          const setter = Object.getOwnPropertyDescriptor(
            field instanceof HTMLTextAreaElement
              ? HTMLTextAreaElement.prototype
              : HTMLInputElement.prototype,
            'value',
          )?.set
          setter?.call(field, value)
          field.dispatchEvent(new Event('input', { bubbles: true }))
          return true
        },
        step.fill,
        step.value ?? '',
      )
      if (!written) fail(`no field ${step.fill} on the way to ${surface.name}`)
      await new Promise((resolve) => setTimeout(resolve, 200))
      continue
    }
    // A `selector` step clicks by CSS, for a control whose label is computed rather than
    // fixed — the syllabus upload's button reads `Read the syllabus ({{count}} credits)`,
    // so a label-keyed step would have to encode the plan's credit cost and would break
    // the day `config/plans.toml` changes it. The four tap kinds stay the default, because
    // a label is what a *student* sees and matching it is the stronger check.
    if (step.selector !== undefined) {
      const clicked = await page.evaluate((selector) => {
        const target = document.querySelector(selector)
        if (target === null) return false
        target.click()
        return true
      }, step.selector)
      if (!clicked) fail(`no element ${step.selector} on the way to ${surface.name}`)
      await new Promise((resolve) => setTimeout(resolve, 400))
      continue
    }
    const wanted = step.nav ?? step.link ?? step.tab ?? step.button
    const scope =
      step.nav !== undefined
        ? 'nav a'
        : step.link !== undefined
          ? 'a'
          : step.tab !== undefined
            ? '[role="tab"]'
            : 'button'
    const click = (label) =>
      page.evaluate(
        (selector, text) => {
          const all = [...document.querySelectorAll(selector)]
          // **A link the screen renders wins over one in the shell's navigation.** The
          // two can carry the same word: `nav.study` labels both the sidebar's Study
          // item and the flashcards panel's "study this deck" action, and at desktop
          // width the sidebar comes first in the DOM — so a plain first-match walked to
          // the Study *hub* and then failed to find a card on it, which reads as a
          // missing control rather than as an ambiguous label. Reordering rather than
          // excluding keeps every step a label: `{ link: 'Calendar' }` still resolves to
          // the planner's switcher, which lives inside a `nav` of its own.
          const ordered =
            selector === 'a'
              ? [
                  ...all.filter((a) => a.closest('nav') === null),
                  ...all.filter((a) => a.closest('nav') !== null),
                ]
              : all
          const link = ordered.find((a) => a.textContent.trim() === text)
          if (link === undefined) return false
          link.click()
          return true
        },
        scope,
        label,
      )
    // The tab bar's five labels are short and the sidebar's are long, so a `nav` step
    // written against the phone ("Work", "More") finds nothing at desktop width. Retrying
    // with the other form is what lets one `SURFACES` table be walked at both viewports,
    // which is what gate 5's second leg needs; the phone labels stay exactly as written.
    //
    // **The five destinations changed, and this table navigates by their labels.** They
    // are now `Today / Workspaces / Study / Planner / More`; Tasks and Calendar became
    // views *inside* Planner, and Study gained a tab it never had. The `SURFACES` steps
    // for calendar, tasks, planner, syllabus and syllabus-review were rewritten to reach
    // those screens through Planner's own switcher — which is also the student's path, so
    // the walk still proves reachability rather than merely that a URL renders.
    //
    // The alias table below is unchanged and still needed: the phone writes `Work` and
    // `More` where the sidebar writes `Workspaces` and `Settings`, and one `SURFACES`
    // table is walked at both viewports. The phone labels stay exactly as written.
    // `Calendar` is the desktop's label for the planner's first view and `Plan` is
    // the phone's (`PlannerScreen.tsx` picks it by form factor, following
    // `SPEC.md:665`'s "Plan | Tasks" for the phone and the design's frame label for
    // the desktop). One `SURFACES` table is walked at both widths, so the pair is an
    // alias like `Work`/`Workspaces` rather than two rows.
    const aliases = { Work: 'Workspaces', More: 'Settings', Calendar: 'Plan' }
    // The two attempts are separated by a wait, and that is load-bearing for a step
    // that follows a navigation: `{ nav: 'Planner' }, { link: 'Calendar' }` clicks the
    // tab, and the retry then has to look for the alias **in the planner's own DOM**,
    // which does not exist until React has re-rendered. Retrying immediately found
    // neither label and failed with "no a labelled Calendar" — a failure that reads as
    // a missing control rather than as a race.
    let found = await click(wanted)
    if (!found && aliases[wanted] !== undefined) {
      await new Promise((resolve) => setTimeout(resolve, 400))
      found = await click(aliases[wanted])
    }
    if (!found) fail(`no ${scope} labelled "${wanted}" on the way to ${surface.name}`)
    await new Promise((resolve) => setTimeout(resolve, 400))
  }

  // A key press that has to *do* something, which the Tab walk cannot express: it
  // measures reach and the focus ring, not that a shortcut acts. `surface.expect` is
  // asserted after this, so the observable effect is the surface's own expectation
  // moving — a key press with no handler leaves the screen as the steps left it and
  // the check fails rather than reporting a pass.
  for (const key of surface.keys ?? []) {
    await page.keyboard.press(key.press)
    await new Promise((resolve) => setTimeout(resolve, 400))
  }
}

/**
 * The keyboard leg: gate 5(a), driven from here because a key press has to happen
 * between two reads of `document.activeElement` and an element reference cannot cross
 * back to Node. `measureInPage` has already stashed the control set and the reads
 * (`window.__gate5Keyboard`); this walks it.
 *
 * **It starts at the top of the document, deliberately.** Chrome keeps the sequential
 * focus navigation start point across a navigation, so the first `Tab` after the
 * `signIn` swap went to the browser chrome on the `today` surface and the walk measured
 * 0 stops for 9 controls. A walk that starts mid-document measures a suffix of the
 * focus order and reports it as the whole, so the start point is reset before the first
 * press (see `resetStart`) and what held focus before the reset is recorded.
 *
 * **The loop's three exits, all of them reported.** It stops when focus returns to the
 * first stop (the natural end of a page's focus order), when focus leaves the document
 * (Chrome's own chrome holds it — also an end), or when the cap is hit. The cap is
 * `4 x controls + 40`: a genuine focus order visits each control about once, so four
 * passes plus a margin is generous, and it is a *function of the set* rather than a
 * constant so a bigger surface does not silently start reporting a cap. **A cap hit is
 * a problem**, not a quieter walk: it means the walk cannot say whether the rest of the
 * set is reachable, and "0 unreached" from a walk that stopped early is the shape of
 * coverage without the substance.
 *
 * **A trap is the previous stop repeating**, with one exemption: a composite date/time
 * input (`input[type="date"]`, `#task-due` and `#cheatsheet-exam-date` here) is several
 * stops to Tab — the platform moves between its internal month/day/year fields and its
 * picker button — while `document.activeElement` stays the host element. Those repeats
 * are counted and reported separately (`compositeRepeats`), and the cap is the backstop
 * that still catches a composite control that never lets go.
 *
 * The page is left as it was found (`restore`), including the blur that keeps a walk
 * ending on the `sr-only` skip link from changing the *target* check's next reading.
 */
async function walkKeyboard(page, key, problems) {
  const expected = await page.evaluate(() => window.__gate5Keyboard.expected)
  const cap = expected * 4 + 40
  const stops = new Map()
  const rings = []
  const traps = []
  const compositeRepeats = []
  let firstId = null
  let previousId = null
  let focusLeft = false
  let capped = false
  let presses = 0

  await page.evaluate(() => {
    window.__gate5Keyboard.captureScroll()
  })
  const startedAt = await page.evaluate(() => window.__gate5Keyboard.resetStart())

  while (presses < cap) {
    await page.keyboard.press('Tab')
    presses += 1
    // Focus is applied synchronously by the press, but `:focus-visible` styling and the
    // scroll-into-view that follows it are a frame later, and the ring is read from
    // computed style. 25ms is one frame on a machine under load; a shorter wait reads
    // the ring of the control the *previous* press focused.
    await new Promise((resolve) => setTimeout(resolve, 25))
    const stop = await page.evaluate(() => window.__gate5Keyboard.read())
    if (stop.none) {
      focusLeft = true
      break
    }
    if (firstId === null) firstId = stop.id
    if (stop.id === previousId) {
      if (stop.composite) compositeRepeats.push({ ...stop, press: presses })
      else traps.push({ ...stop, press: presses })
      // Keep going: the cap bounds the loop, and the walk still has to report whether
      // the rest of the set is reachable.
    }
    previousId = stop.id
    if (!stops.has(stop.id)) {
      stops.set(stop.id, stop)
      if (!stop.ringOk) rings.push(stop)
    }
    if (stop.id === firstId && stops.size > 1) break
  }
  // The cap is a finding rather than a quiet truncation: it means the walk cannot say
  // whether the rest of the set is reachable. `focusLeft` and the return-to-first exit
  // are the other two ways out, and both are reported in the line below.
  if (presses >= cap && !focusLeft) capped = true

  const { unreached, excused, groups, focusStops } = await page.evaluate(() =>
    window.__gate5Keyboard.summary(),
  )
  const info = new Map(
    (await page.evaluate(() => window.__gate5Keyboard.info())).map((entry) => [entry.id, entry]),
  )
  const named = (id) => info.get(id) ?? { id, el: '?', text: '' }

  // The groups the exemptions stand on, measured rather than assumed. Only a group with
  // a member Tab reached is here, so this can only *add* findings — the group's
  // reachability itself was already checked by the walk.
  const arrowReached = []
  for (const group of groups) {
    const landed = await page.evaluate((id) => window.__gate5Keyboard.focusStop(id), group.focus)
    if (!landed) {
      problems.push(
        `${key} keyboard: the ${group.reason} group's Tab stop could not be focused to test it`,
      )
      continue
    }
    const reached = new Set([group.focus])
    // Both directions: the tablists wrap (ArrowRight cycles the strip), the radio group
    // does not (ArrowDown from the middle reaches the ones below it, ArrowUp the ones
    // above), so one direction alone would report a false failure on a radio group.
    const arrows = group.reason === 'radio' ? ['ArrowDown', 'ArrowUp'] : ['ArrowRight', 'ArrowLeft']
    for (const arrow of arrows) {
      for (let step = 0; step < group.members; step++) {
        await page.keyboard.press(arrow)
        await new Promise((resolve) => setTimeout(resolve, 25))
        const id = await page.evaluate(() => window.__gate5Keyboard.focused())
        if (id === -1) break
        reached.add(id)
      }
    }
    const missing = group.ids.filter((id) => !reached.has(id))
    arrowReached.push({
      reason: group.reason,
      members: group.members,
      reachedByTab: group.reachedByTab,
      reachedByArrow: group.members - missing.length,
    })
    for (const id of missing) {
      const item = named(id)
      problems.push(
        `${key} keyboard: ${item.el} "${item.text}" is a ${group.reason} group member that neither Tab nor the arrow keys reach`,
      )
    }
  }

  await page.evaluate(() => {
    window.__gate5Keyboard.restore()
  })

  const where = `${key} keyboard`
  const exit = focusLeft
    ? 'focus then left the page'
    : capped
      ? `stopped at the ${String(cap)}-press cap`
      : 'focus returned to the first stop'
  console.log(
    `  ${where}: ${String(stops.size)} Tab stops for ${String(expected)} controls; ` +
      `${String(unreached.length)} unreached; ${String(excused.length)} excused by design; ` +
      `${String(rings.length)} with no focus ring; ${String(traps.length)} trapping; ${exit}`,
  )
  for (const group of arrowReached) {
    console.log(
      `    ${where}: ${group.reason} group of ${String(group.members)} — ${String(group.reachedByTab)} reached by Tab, ${String(group.reachedByArrow)} by arrow keys`,
    )
  }
  if (compositeRepeats.length > 0) {
    console.log(
      `    ${where}: ${String(compositeRepeats.length)} presses stayed inside a composite date/time input (its own internal fields)`,
    )
  }
  for (const item of unreached) {
    problems.push(
      `${where}: ${item.el} "${item.text}" is never reached by Tab (${String(focusStops)} Tab stops for ${String(expected)} controls)`,
    )
  }
  for (const item of rings) {
    problems.push(
      `${where}: ${item.el} "${item.text}" shows no focus indicator when focused (outline: ${item.outline}; box-shadow: ${item.shadow})`,
    )
  }
  for (const item of traps) {
    problems.push(
      `${where}: ${item.el} "${item.text}" keeps focus after Tab (press ${String(item.press)}); a keyboard user cannot get past it`,
    )
  }
  if (capped) {
    problems.push(
      `${where}: the Tab walk hit its ${String(cap)}-press cap after ${String(stops.size)} Tab stops with ${String(unreached.length)} of ${String(expected)} controls unreached`,
    )
  }

  return {
    expected,
    stops: [...stops.values()].map(({ id, el, text, outline, shadow }) => ({
      id,
      el,
      text,
      outline,
      shadow,
    })),
    startedAt,
    presses,
    focusLeft,
    capped,
    cap,
    unreached,
    excused,
    arrowGroups: arrowReached,
    compositeRepeats: compositeRepeats.map(({ el, text, press }) => ({ el, text, press })),
    rings: rings.map(({ el, text, outline, shadow }) => ({ el, text, outline, shadow })),
    traps: traps.map(({ el, text, press }) => ({ el, text, press })),
  }
}

async function main() {
  const chrome = findChrome()
  if (chrome === null) {
    fail(
      `no Chromium found; set CHROME_PATH (looked in ${CHROME_CANDIDATES.join(', ')})`,
    )
  }

  // Fail before launching if nothing is serving the app: a connection error from
  // deep inside puppeteer reads like a bug in this script.
  try {
    const response = await fetch(APP_URL)
    if (!response.ok) fail(`${APP_URL} answered ${response.status}`)
  } catch (error) {
    fail(`${APP_URL} is not reachable (${error.message}) — start the dev server first`)
  }

  console.log(
    `measuring ${APP_URL} at ${String(VIEWPORT.width)}x${String(VIEWPORT.height)} at ${SCALES.map((scale) => `${String(scale)}%`).join('/')} text with ${chrome}`,
  )

  const browser = await puppeteer.launch({
    executablePath: chrome,
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  })

  const results = []
  const problems = []

  // The surfaces this run walks: the leg's own, or the subset named by `ONLY_SURFACES`.
  // A name that matches nothing is a failure rather than an empty walk, because
  // `ONLY_SURFACES=stduy` would otherwise report a clean run over zero screens.
  //
  // `desktopOnly` is dropped on the phone leg, and the *reason* is in the surface's own
  // comment: a screen a phone does not render cannot be measured at 390px, and walking it
  // there would measure whatever the phone renders instead and report a PASS. The
  // denominator below is this list rather than `SURFACES`, so neither leg can claim
  // coverage of a surface it cannot reach.
  const available = SURFACES.filter(
    (surface) => surface.desktopOnly !== true || MIN_TARGET === DESKTOP_TARGET,
  )
  const surfaces =
    ONLY === undefined
      ? available
      : available.filter((surface) => ONLY.split(',').includes(surface.name))
  if (surfaces.length === 0) {
    fail(`ONLY_SURFACES=${ONLY} matched no surface`)
  }
  // A `keys` step's only observable effect is the surface's `expect` moving, so a surface
  // with keys and no expectation presses a key and asserts nothing — the vacuity this
  // script's other guards exist to prevent. Caught here, where the tables are validated,
  // rather than by noticing an absent result later.
  for (const surface of surfaces) {
    if (surface.keys !== undefined && surface.expect === undefined) {
      fail(`${surface.name} has a \`keys\` step but no \`expect\`, so the key press asserts nothing`)
    }
  }

  for (const theme of ['dark', 'light']) {
    for (const scale of SCALES) {
      const page = await browser.newPage()
      await page.setViewport(VIEWPORT)
      await page.emulateMediaFeatures([{ name: 'prefers-color-scheme', value: theme }])
      await page.setRequestInterception(true)

      const seen = new Set()
      page.on('request', (request) => {
        const url = new URL(request.url())
        if (url.origin === new URL(APP_URL).origin) return request.continue()
        seen.add(`${request.method()} ${url.pathname}`)
        const respond = (status, body) =>
          request.respond({
            status,
            headers: {
              'access-control-allow-origin': '*',
              'access-control-allow-headers': '*',
              'access-control-allow-methods': '*',
              'content-type': 'application/json',
            },
            body: JSON.stringify(body),
          })
        // The preflight carries the same allow-headers as the answer it precedes:
        // the app sends `authorization` and `content-type`, both of which are
        // non-simple, so a 204 without `access-control-allow-headers` fails the
        // check in the browser and every request after it is never sent.
        if (request.method() === 'OPTIONS') {
          return request.respond({
            status: 204,
            headers: {
              'access-control-allow-origin': '*',
              'access-control-allow-headers': '*',
              'access-control-allow-methods': '*',
            },
            body: '',
          })
        }
        const handler = routeFor(url.pathname)
        if (handler === undefined) return respond(404, { code: 'not_found', message: 'no' })
        // The checkout screen's `offline` state is "the request never arrived", and the
        // only way to produce it is to make the request fail at the network layer. A 500
        // would render `waiting` with an error notice, which is a different screen.
        if (CHECKOUT.offline && url.pathname === `/billing/payments/${CHECKOUT.id}`) {
          return request.abort('failed')
        }
        return respond(200, handler())
      })

      await page.goto(APP_URL, { waitUntil: 'domcontentloaded' })
      await signIn(page)

      // The shell's own fonts settle here, before the per-surface wait repeats it: the
      // first surface would otherwise be the one that pays the load.
      await page.evaluate(() => document.fonts.ready.then(() => undefined))

      // The root font size, set the way the reader's own setting does. 32px is
      // 200% of the 16px default and is what every phase's gate has used.
      await page.evaluate((px) => {
        document.documentElement.style.fontSize = `${String(px)}px`
      }, (16 * scale) / 100)
      await new Promise((resolve) => setTimeout(resolve, 300))

      for (const surface of surfaces) {
        await openSurface(page, surface)
        // **Wait for the webfonts before measuring anything.**
        //
        // The app self-hosts Plus Jakarta Sans and JetBrains Mono, and a heading measured
        // before they arrive is measured in the *fallback* face — which is narrower, so a
        // screen that overflows with the real font can fit with the fallback and report a
        // PASS. That is not hypothetical: the redesign's `h1 { font-size: 2rem;
        // font-family: var(--font-heading) }` makes this the first time it matters, and it
        // is what made `workspaces` pass at two combinations and fail at the other two in
        // the same run — the walk was measuring two different fonts.
        //
        // `document.fonts.ready` resolves when the pending font loads settle. It is awaited
        // after every navigation *and* after the root font-size change above, because a
        // late-arriving face re-lays-out text at any size.
        //
        // **This also makes every previously recorded number comparable**, which is the
        // larger point: a gate whose result depends on a font race is not a gate. The
        // numbers in the plans were taken before this wait existed, so any that move now
        // moved for this reason as much as for the surface list.
        await page.evaluate(() => document.fonts.ready.then(() => undefined))
        await new Promise((resolve) => setTimeout(resolve, 100))
        // **The resting state, not a hover.** Every step above ends in a click, so the
        // pointer is left sitting on the control that was clicked — and after a navigation
        // that coordinate is very often over a *different* control on the screen just
        // reached. Measuring there reads `:hover` styles, which is not the state a reader
        // arrives in, and it reported a false contrast failure on the pricing page's
        // "Choose Pro" (white on `color-mix(in oklab, …)`, which `parseColor` cannot read,
        // so the walk fell through to the card behind it). Moving the pointer to the corner
        // makes every measurement the one a reader sees before touching anything.
        await page.mouse.move(1, 1)
        await new Promise((resolve) => setTimeout(resolve, 150))

        // **The surface rendered the screen it names.** The aggregate guard at the bottom of
        // this function only catches a walk that measured *nothing*; a surface whose fixture
        // 404s still renders a shell with a notice, which has text pairs and targets — so it
        // passes the aggregate while measuring the wrong screen. `expect` is a string the
        // screen must show, and its absence is a failure rather than a smaller number.
        if (surface.expect !== undefined) {
          // A **wait**, not a sample: the offline state arrives when the poll's request is
          // aborted, which is one round trip after the navigation, so a single read races it
          // and reports a screen that had simply not settled. Ten seconds is generous for a
          // fixture answered in-process, and a timeout here is a real failure.
          let text = ''
          try {
            await page.waitForFunction(
              (wanted) => document.body.innerText.includes(wanted),
              { timeout: 10000 },
              surface.expect,
            )
          } catch {
            // Fall through to the read below, which reports what was on the page instead.
          }
          text = await page.evaluate(() => document.body.innerText)
          if (!text.includes(surface.expect)) {
            problems.push(
              `${surface.name}/${theme}/x${String(scale)} did not render its screen: ${JSON.stringify(surface.expect)} is absent from the page`,
            )
            continue
          }
        }

        const measured = await page.evaluate(measureInPage, {
          minTarget: MIN_TARGET,
          // The inline-citation exemption below is a **desktop** one (see its comment):
          // it applies only when the floor is the spec's 32px desktop value, never when
          // it is the 44px touch value. Passed as a flag rather than compared against a
          // constant, because `measureInPage` runs inside the page and cannot see this
          // module's bindings.
          desktopFloor: MIN_TARGET === DESKTOP_TARGET,
          minText: MIN_TEXT,
          minLarge: MIN_LARGE,
        })
        const key = `${surface.name}/${theme}/x${String(scale)}`
        // Gate 5(a), on the control set `measureInPage` just collected — the walk is run
        // here rather than inside that function because a real key press has to happen
        // between two reads of `document.activeElement`. It restores the page and blurs
        // before returning, so the `words` check below and the *next* surface both see
        // the resting state.
        const keyboard = await walkKeyboard(page, key, problems)
        results.push({ key, ...measured, keyboard, seen: [...seen] })

        console.log(
          `${key}: ${String(measured.targets.length)} targets, ${String(measured.undersized.length)} under ${String(MIN_TARGET)}px; ` +
            `${String(measured.contrasts.length)} text pairs, ${String(measured.below.length)} below requirement; ` +
            `${String(measured.clipped.length)} clipped; ${String(measured.overlaps.length)} overlapped; ` +
            `page ${String(measured.pageOverflow.scrollWidth)}/${String(measured.pageOverflow.clientWidth)}`,
        )

        for (const target of measured.undersized) {
          problems.push(
            `${key} target ${target.el} "${target.text}" is ${String(target.w)}x${String(target.h)}`,
          )
        }
        for (const pair of measured.below) {
          problems.push(
            `${key} contrast ${pair.el} "${pair.text}" ${String(pair.ratio)}:1 needs ${String(pair.need)}:1 (${pair.color} on ${pair.bg})`,
          )
        }
        for (const item of measured.clipped) {
          problems.push(
            `${key} clipped ${item.el} "${item.text}" scroll ${String(item.scrollWidth)}x${String(item.scrollHeight)} vs client ${String(item.clientWidth)}x${String(item.clientHeight)}`,
          )
        }
        for (const item of measured.overlaps) {
          problems.push(`${key} overlapped ${item.el} "${item.text}" by ${item.coveredBy} at ${item.at}`)
        }
        for (const item of measured.pastEdge) {
          problems.push(
            `${key} painted past the ${item.side} edge: ${item.el} "${item.text}" is ${String(item.width)}px wide, ${String(item.past)}px outside the viewport`,
          )
        }
        if (measured.pageOverflow.overflows) {
          problems.push(
            `${key} the page scrolls sideways: ${String(measured.pageOverflow.scrollWidth)} > ${String(measured.pageOverflow.clientWidth)}`,
          )
        }

        // Gate 5(d), measured in the browser: the state is never colour alone. Each
        // checkout state names itself in a chip word and a title, and both have to be in
        // the rendered text — a status carried only by a pill's tone is the likeliest
        // defect on a screen whose whole subject is whether money moved. Asserted on
        // `body.innerText`, which is what a reader (and a screen reader) actually gets.
        if (surface.words !== undefined) {
          const text = await page.evaluate(() => document.body.innerText)
          const missing = surface.words.filter((word) => !text.includes(word))
          if (missing.length > 0) {
            problems.push(
              `${key} the state is not in words: missing ${JSON.stringify(missing)} from the rendered text`,
            )
          } else {
            console.log(`  ${key} states itself in words: ${JSON.stringify(surface.words)}`)
          }
        }
      }

      await page.close()
    }
  }

  await browser.close()

  // The measurement must have measured something. A walk that found no target and
  // no text pair has proved nothing, and reporting "0 failures" from it would be
  // the same defect the purchase-UI check guards against.
  //
  // **The collected problems are printed first**, because a run where every surface
  // failed its own `expect` guard reaches this line with an empty result set — so the
  // guard's "did not reach a rendered screen" is true but names the wrong cause, and
  // the four lines that say which surface rendered what are exactly what the reader
  // needs. A red run whose message points at the wrong thing costs more than the red.
  const totalTargets = results.reduce((sum, entry) => sum + entry.targets.length, 0)
  const totalPairs = results.reduce((sum, entry) => sum + entry.contrasts.length, 0)
  if (totalTargets === 0 || totalPairs === 0) {
    if (problems.length > 0) {
      console.error(`\n${String(problems.length)} problem(s) before the walk stopped measuring:`)
      for (const problem of problems) console.error(`  ${problem}`)
    }
    fail(
      `measured ${String(totalTargets)} targets and ${String(totalPairs)} text pairs — the walk did not reach a rendered screen`,
    )
  }

  // The keyboard leg's own version of the same guard, and it is not redundant: a page
  // whose controls are all unreachable reports zero focus stops and a long list of
  // unreached controls, but a leg that was never *driven* (no presses, no reads) would
  // report zero stops and zero unreached — which reads as a pass. The stops are what
  // prove the key presses happened.
  const totalStops = results.reduce((sum, entry) => sum + entry.keyboard.stops.length, 0)
  const totalUnreached = results.reduce((sum, entry) => sum + entry.keyboard.unreached.length, 0)
  const totalExcused = results.reduce((sum, entry) => sum + entry.keyboard.excused.length, 0)
  const totalRingless = results.reduce((sum, entry) => sum + entry.keyboard.rings.length, 0)
  const totalTraps = results.reduce((sum, entry) => sum + entry.keyboard.traps.length, 0)
  if (totalStops === 0) {
    fail(
      `the keyboard leg reached ${String(totalStops)} Tab stops over ${String(results.length)} combinations — the Tab walk was never driven`,
    )
  }

  const output = join(root, 'var', process.env.OUTPUT ?? 'gate14-phone.json')
  try {
    execFileSync('mkdir', ['-p', dirname(output)])
  } catch {
    // `mkdir -p` failing means the directory already exists; the write below says so.
  }
  const { writeFileSync } = await import('node:fs')
  writeFileSync(output, `${JSON.stringify({ viewport: VIEWPORT, scales: SCALES, results }, null, 1)}\n`)

  const lowest = Math.min(...results.flatMap((entry) => entry.contrasts.map((pair) => pair.ratio)))
  console.log(
    `\n${String(results.length)} screen/theme/scale combinations: ${String(totalTargets)} targets, ` +
      `${String(totalPairs)} text pairs, lowest contrast ${String(lowest)}:1; ` +
      `${String(totalStops)} keyboard Tab stops, ${String(totalUnreached)} unreached, ` +
      `${String(totalExcused)} excused by design, ${String(totalRingless)} without a focus ring, ` +
      `${String(totalTraps)} trapping`,
  )
  console.log(`full report: ${output}`)

  // The verdict names the viewport, because this walk has two legs now and the
  // sentence it printed before was wrong on the desktop one: it said "on a phone
  // viewport" at 1365px, which is the same defect class as a Result column that
  // holds a claim the run did not make. The keyboard leg's totals are on it too, so
  // the one line that says PASS says which legs it is passing. **The failing path
  // prints the same line with FAIL**, because "which run was this" is exactly the
  // question a red run leaves a reader with, and the previous version answered it
  // only when the answer was yes.
  const leg = MIN_TARGET === DESKTOP_TARGET ? 'desktop' : 'phone'
  // A filtered run is not a gate result, and the one line that says PASS has to say so:
  // "gate 5 PASS" from a walk of one surface is exactly the shape of coverage without
  // the substance this script's other guards exist to prevent.
  const coverage =
    ONLY === undefined
      ? `all ${String(surfaces.length)} surfaces`
      : `${String(surfaces.length)} of ${String(available.length)} surfaces (ONLY_SURFACES=${ONLY}) — NOT a full gate run`
  const verdict = (result) =>
    `gate 5 on a ${leg} viewport: ${result} (${String(VIEWPORT.width)}x${String(VIEWPORT.height)}, ${String(SCALES.length)} scale(s), both themes; ` +
    `${coverage}; keyboard: ${String(totalStops)} Tab stops, ${String(totalUnreached)} unreached, ${String(totalExcused)} excused, ` +
    `${String(totalRingless)} without a ring, ${String(totalTraps)} trapping)`

  if (problems.length > 0) {
    console.error(`\n${String(problems.length)} problems:`)
    for (const problem of problems) console.error(`  ${problem}`)
    console.error(`\n${verdict(`FAIL, ${String(problems.length)} problem(s)`)}`)
    process.exit(1)
  }

  console.log(verdict('PASS'))
}

await main()
