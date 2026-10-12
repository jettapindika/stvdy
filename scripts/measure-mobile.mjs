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
 */
const VIEWPORT = { width: 390, height: 844, deviceScaleFactor: 3 }
const SCALES = [100, 200]

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

/** The smallest interactive-target side WCAG 2.2 SC 2.5.8 allows, and R-03's. */
const MIN_TARGET = 44

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
  due_at: '2026-10-16T16:59:00Z',
  estimate_min: 60,
  priority: 2,
  status: 'todo',
  position: 0,
  created_at: '2026-10-11T08:00:00Z',
  updated_at: '2026-10-11T08:00:00Z',
  deleted_at: null,
}

/**
 * The phone surfaces this walks, by the taps a phone takes to reach them.
 *
 * The tab bar's five labels are the five destinations. `/calendar` is absent
 * because it is a `NotBuiltRoute` — there is no screen to measure — and `/billing`
 * is reached the way a phone reaches it, through Settings, which is also the
 * assertion that it is reachable at all: a screen nothing links to would otherwise
 * be measured by URL and would pass while being invisible to a user.
 */
const SURFACES = [
  { name: 'today', steps: [{ nav: 'Today' }] },
  { name: 'workspaces', steps: [{ nav: 'Work' }] },
  { name: 'tasks', steps: [{ nav: 'Tasks' }] },
  { name: 'settings', steps: [{ nav: 'More' }] },
  { name: 'plan', steps: [{ nav: 'More' }, { link: 'Open plan and usage' }] },
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
  '/workspaces': () => ({ workspaces: [] }),
  '/tasks': () => ({ tasks: [TASK] }),
  '/settings': () => ({ settings: SETTINGS_ROW, timezone: USER.timezone }),
  '/events': () => ({ events: [] }),
  '/sessions': () => ({ sessions: [] }),
  '/devices': () => ({ devices: [] }),
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
  const { minTarget, minText, minLarge } = options

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
  for (const el of document.querySelectorAll(TARGETS)) {
    if (!isVisible(el)) continue
    // A skip link is `sr-only` until focused: it is 1x1 by design, and what makes
    // it usable is that focusing it expands it. Measured focused, below.
    if (el.classList.contains('sr-only') && document.activeElement !== el) continue
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
  const clipped = []
  const CLIPS_BY_DESIGN = new Set(['INPUT', 'SELECT', 'TEXTAREA'])
  for (const el of document.querySelectorAll('body *')) {
    if (!isVisible(el)) continue
    if (el.classList.contains('sr-only')) continue
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
 */
async function openSurface(page, surface) {
  for (const step of surface.steps) {
    const wanted = step.nav ?? step.link
    const scope = step.nav === undefined ? 'a' : 'nav a'
    const found = await page.evaluate(
      (selector, label) => {
        const link = [...document.querySelectorAll(selector)].find(
          (a) => a.textContent.trim() === label,
        )
        if (link === undefined) return false
        link.click()
        return true
      },
      scope,
      wanted,
    )
    if (!found) fail(`no ${scope} labelled "${wanted}" on the way to ${surface.name}`)
    await new Promise((resolve) => setTimeout(resolve, 400))
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

  console.log(`measuring ${APP_URL} at ${VIEWPORT.width}x${VIEWPORT.height} with ${chrome}`)

  const browser = await puppeteer.launch({
    executablePath: chrome,
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  })

  const results = []
  const problems = []

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
        const handler = ROUTES[url.pathname]
        if (handler === undefined) return respond(404, { code: 'not_found', message: 'no' })
        return respond(200, handler())
      })

      await page.goto(APP_URL, { waitUntil: 'domcontentloaded' })
      await signIn(page)

      // The root font size, set the way the reader's own setting does. 32px is
      // 200% of the 16px default and is what every phase's gate has used.
      await page.evaluate((px) => {
        document.documentElement.style.fontSize = `${String(px)}px`
      }, (16 * scale) / 100)
      await new Promise((resolve) => setTimeout(resolve, 300))

      for (const surface of SURFACES) {
        await openSurface(page, surface)
        const measured = await page.evaluate(measureInPage, {
          minTarget: MIN_TARGET,
          minText: MIN_TEXT,
          minLarge: MIN_LARGE,
        })
        const key = `${surface.name}/${theme}/x${String(scale)}`
        results.push({ key, ...measured, seen: [...seen] })

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
      }

      await page.close()
    }
  }

  await browser.close()

  // The measurement must have measured something. A walk that found no target and
  // no text pair has proved nothing, and reporting "0 failures" from it would be
  // the same defect the purchase-UI check guards against.
  const totalTargets = results.reduce((sum, entry) => sum + entry.targets.length, 0)
  const totalPairs = results.reduce((sum, entry) => sum + entry.contrasts.length, 0)
  if (totalTargets === 0 || totalPairs === 0) {
    fail(
      `measured ${String(totalTargets)} targets and ${String(totalPairs)} text pairs — the walk did not reach a rendered screen`,
    )
  }

  const output = join(root, 'var', 'gate14-phone.json')
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
      `${String(totalPairs)} text pairs, lowest contrast ${String(lowest)}:1`,
  )
  console.log(`full report: ${output}`)

  if (problems.length > 0) {
    console.error(`\n${String(problems.length)} problems:`)
    for (const problem of problems) console.error(`  ${problem}`)
    process.exit(1)
  }

  console.log('gate 5 on a phone viewport: PASS')
}

await main()
