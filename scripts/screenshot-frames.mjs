#!/usr/bin/env node
/**
 * Screenshots the app at a Figma frame's own viewport, for side-by-side comparison.
 *
 * **Why this is not part of the gate.** `measure-mobile.mjs` measures; it does not
 * look. Gate 5 answers "is any target undersized, any text unreadable, anything
 * clipped" — questions with numbers for answers. "Does this screen match the frame"
 * is a different question, and the only instrument for it is an image of the screen
 * at the frame's exact width beside an image of the frame.
 *
 * The frames are 1440, 1680 and 1760 wide; the gate walks 390 and 1365. So a
 * screenshot has to be taken at the design's width, with the design's data, in the
 * design's theme — and that is what this does, reusing `measure-mobile.mjs`'s
 * fixtures rather than restating them.
 *
 * Usage:
 *
 *   node scripts/screenshot-frames.mjs                       # every mapped frame
 *   node scripts/screenshot-frames.mjs today-desktop         # named frames only
 *   node scripts/screenshot-frames.mjs --list                # show the table
 *
 * `APP_URL` defaults to `http://localhost:5173`; `OUT` defaults to `var/frames`.
 */
import { mkdir } from 'node:fs/promises'
import { join } from 'node:path'
import { pathToFileURL } from 'node:url'
import {
  APP_URL,
  ROUTES,
  SURFACES,
  VIEWPORT,
  findChrome,
  openSurface,
  puppeteer,
  root,
  routeFor,
  signIn,
} from './measure-mobile.mjs'

const OUT = join(root, process.env.OUT ?? 'var/frames')

/**
 * A Figma frame, and how to reach the screen it draws.
 *
 * `width`/`height` are the frame's own — the number in the design file, which is what
 * a comparison needs. `theme` is the frame's; 53 of the 54 are light and the dark
 * workspace is the exception. `surface` names an entry in `SURFACES`, so the steps
 * that reach the screen are the gate's own and cannot drift from it; `steps` is the
 * override for the frames no surface covers (the pre-auth screens, the palette).
 *
 * `dpr: 2` is what makes a 1440px frame comparable at all: a 1x screenshot of 1440
 * CSS px is a 1440px image, and the design's 1px hairlines and 11px metadata are
 * what the comparison is about.
 */
const FRAMES = [
  // --- A. Foundations ------------------------------------------------------------
  // The three design-system sheets are `/styleguide`, which renders the same tokens
  // the sheets document. It is one screen against three frames, so it is captured
  // once per sheet's own width and the sheets are compared section by section.
  { name: 'foundations', figma: '411:17813', width: 1440, height: 2931, url: '/styleguide' },
  { name: 'ui-controls', figma: '411:18149', width: 1440, height: 2200, url: '/styleguide' },
  {
    name: 'academic-patterns',
    figma: '411:18675',
    width: 1440,
    height: 2490,
    url: '/styleguide',
  },

  // --- B. Auth and settings -------------------------------------------------------
  // The four auth screens are pre-session, so they are reached by URL and captured
  // before `signIn` rather than by a surface step.
  { name: 'sign-in-desktop', figma: '411:24008', width: 1440, height: 900, preAuth: '/signin' },
  { name: 'sign-in-mobile', figma: '411:24046', width: 390, height: 844, preAuth: '/signin' },
  { name: 'sign-up-desktop', figma: '411:24090', width: 1440, height: 900, preAuth: '/signup' },
  { name: 'sign-up-mobile', figma: '411:24128', width: 390, height: 844, preAuth: '/signup' },
  { name: 'settings-devices', figma: '419:3234', width: 1440, height: 1080, url: '/settings/devices' },
  {
    name: 'settings-calendar',
    figma: '419:3383',
    width: 1440,
    height: 1139,
    url: '/settings/calendar',
  },

  // --- C. Core app ----------------------------------------------------------------
  { name: 'today-desktop', figma: '411:26794', width: 1440, height: 1466, surface: 'today' },
  // The frame's own state, which the entry above cannot show: the walk's fixtures put the
  // event off the anchor's day and the task four days out, so `today-desktop` is a
  // photograph of the empty timeline. `411:26794` draws four rows, so the comparison
  // against *that* frame is this one.
  {
    name: 'today-populated-desktop',
    figma: '411:26794',
    width: 1440,
    height: 1466,
    surface: 'today-populated',
  },
  { name: 'today-mobile', figma: '411:27057', width: 390, height: 2965, surface: 'today' },
  {
    name: 'workspaces-desktop',
    figma: '411:43639',
    width: 1440,
    height: 1249,
    surface: 'workspaces',
  },
  {
    name: 'workspaces-mobile',
    figma: '411:43893',
    width: 390,
    height: 2977,
    surface: 'workspaces',
  },
  {
    name: 'workspace-sources-desktop',
    figma: '411:44115',
    width: 1440,
    height: 1443,
    surface: 'workspace-sources',
  },
  {
    name: 'workspace-studio-desktop',
    figma: '411:44378',
    width: 1440,
    height: 1527,
    surface: 'workspace-studio-flashcards',
  },
  {
    name: 'workspace-chat-mobile',
    figma: '411:44683',
    width: 390,
    height: 1657,
    surface: 'workspace-chat',
  },
  {
    name: 'workspace-sources-mobile',
    figma: '411:44801',
    width: 390,
    height: 1399,
    surface: 'workspace-sources',
  },
  {
    name: 'workspace-studio-mobile',
    figma: '411:44923',
    width: 390,
    height: 1695,
    surface: 'workspace-studio-flashcards',
  },
  {
    name: 'study-light',
    figma: '411:18989',
    width: 1440,
    height: 1421,
    surface: 'study-hub',
    theme: 'light',
  },
  {
    name: 'study-dark',
    figma: '411:19278',
    width: 1440,
    height: 1421,
    surface: 'study-hub',
    theme: 'dark',
  },

  // --- D. Study tools -------------------------------------------------------------
  {
    name: 'flashcards-desktop-front',
    figma: '411:58080',
    width: 1440,
    height: 1080,
    surface: 'study',
  },
  {
    name: 'flashcards-desktop-revealed',
    figma: '411:58134',
    width: 1440,
    height: 1080,
    surface: 'study-revealed',
  },
  {
    name: 'flashcards-mobile-front',
    figma: '411:58213',
    width: 390,
    height: 844,
    surface: 'study',
  },
  {
    name: 'flashcards-mobile-revealed',
    figma: '411:58271',
    width: 390,
    height: 898,
    surface: 'study-revealed',
  },
  {
    name: 'quiz-desktop',
    figma: '411:58354',
    width: 1440,
    height: 1080,
    surface: 'workspace-studio-quiz',
  },
  {
    name: 'reports-outline-desktop',
    figma: '411:59161',
    width: 1440,
    height: 1388,
    surface: 'workspace-studio-report',
  },

  // --- E. Planner (1680 wide) -----------------------------------------------------
  { name: 'planner-plan', figma: '411:70512', width: 1680, height: 2197, surface: 'planner' },
  { name: 'planner-tasks-list', figma: '411:70984', width: 1680, height: 1528, surface: 'tasks' },
  {
    name: 'planner-tasks-board',
    figma: '411:71351',
    width: 1680,
    height: 2017,
    surface: 'tasks-board',
  },
  {
    name: 'planner-ai-breakdown',
    figma: '411:71745',
    width: 1680,
    height: 1528,
    surface: 'syllabus',
  },

  // --- F. Billing -----------------------------------------------------------------
  { name: 'pricing-desktop', figma: '420:5714', width: 1440, height: 1080, surface: 'pricing' },
  {
    name: 'checkout-desktop',
    figma: '420:5926',
    width: 1440,
    height: 1080,
    surface: 'checkout-waiting',
  },
  { name: 'plan-mobile', figma: '420:6238', width: 390, height: 844, surface: 'plan' },

  // --- G. System states -----------------------------------------------------------
  { name: 'admin-dashboard', figma: '420:22013', width: 1440, height: 2285, surface: 'admin' },
  {
    name: 'today-offline',
    figma: '420:21268',
    width: 1440,
    height: 1545,
    offline: true,
    steps: [{ nav: 'Workspaces' }, { nav: 'Today' }],
  },
]

const byName = new Map(SURFACES.map((surface) => [surface.name, surface]))

/** Resolve a frame's surface entry, failing loudly rather than screenshotting nothing. */
function surfaceFor(frame) {
  if (frame.steps !== undefined) return { name: frame.name, steps: frame.steps }
  const surface = byName.get(frame.surface)
  if (surface === undefined) {
    throw new Error(`${frame.name}: no surface named ${String(frame.surface)}`)
  }
  return surface
}

/**
 * The route table, with `offline` replacing every answer with a network failure.
 *
 * "Today offline" is the screen the app shows when the API cannot be reached, and a
 * 500 would render the error notice instead — a different screen. `request.abort` is
 * the only way to produce a request that never arrived.
 *
 * `state` is mutable on purpose: the offline frame has to **sign in first**, because
 * aborting the login request too leaves the harness waiting on a form that can never
 * submit. The flag is flipped once the session exists.
 */
function respondWith(page, state) {
  page.on('request', (request) => {
    const url = new URL(request.url())
    if (url.origin === new URL(APP_URL).origin) return request.continue()
    const headers = {
      'access-control-allow-origin': '*',
      'access-control-allow-headers': '*',
      'access-control-allow-methods': '*',
    }
    if (request.method() === 'OPTIONS') {
      return request.respond({ status: 204, headers, body: '' })
    }
    if (state.offline) return request.abort('failed')
    const handler = routeFor(url.pathname)
    if (handler === undefined) {
      return request.respond({
        status: 404,
        headers: { ...headers, 'content-type': 'application/json' },
        body: JSON.stringify({ code: 'not_found', message: 'no' }),
      })
    }
    return request.respond({
      status: 200,
      headers: { ...headers, 'content-type': 'application/json' },
      body: JSON.stringify(handler()),
    })
  })
}

async function main() {
  const args = process.argv.slice(2)
  if (args.includes('--list')) {
    for (const frame of FRAMES) {
      console.log(
        `${frame.name.padEnd(28)} figma=${frame.figma.padEnd(11)} ${String(frame.width)}x${String(frame.height)}`,
      )
    }
    return
  }
  const wanted = args.filter((arg) => !arg.startsWith('--'))
  const frames = wanted.length === 0 ? FRAMES : FRAMES.filter((f) => wanted.includes(f.name))
  if (frames.length === 0) {
    console.error(`no frame matched: ${wanted.join(', ')}`)
    process.exit(1)
  }

  const chrome = findChrome()
  if (chrome === null) {
    console.error('no Chromium found; set CHROME_PATH')
    process.exit(1)
  }
  await mkdir(OUT, { recursive: true })

  const browser = await puppeteer.launch({
    executablePath: chrome,
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  })

  for (const frame of frames) {
    const page = await browser.newPage()
    /**
     * The viewport is the frame's own size, **not clamped**.
     *
     * The shell is a fixed-height flex layout (`h-dvh` with the page scrolling inside
     * `main`), so a viewport shorter than the frame makes the internal scroll container
     * as tall as the viewport — `fullPage` then captures 1600px of a 2977px screen and
     * the bottom two thirds are silently missing. Three frames came back at exactly the
     * clamp height before this was fixed, which is what made it visible.
     */
    const viewport = {
      width: frame.width,
      height: frame.height,
      deviceScaleFactor: 2,
    }
    await page.setViewport(viewport)
    await page.emulateMediaFeatures([
      { name: 'prefers-color-scheme', value: frame.theme ?? 'light' },
    ])
    await page.setRequestInterception(true)
    const state = { offline: false }
    respondWith(page, state)

    try {
      await page.goto(APP_URL, { waitUntil: 'domcontentloaded' })

      if (frame.preAuth !== undefined) {
        // The auth screens are pre-session, so they are reached by URL — the one case
        // where a URL is the honest path, because there is no shell to click through.
        //
        // `goto` rather than `pushState`: the router reads the location at mount, and a
        // programmatic history entry after mount rendered the *same* screen for both
        // `/signin` and `/signup` — byte-identical captures, which is what caught it.
        await page.goto(`${APP_URL}${frame.preAuth}`, { waitUntil: 'domcontentloaded' })
        await new Promise((resolve) => setTimeout(resolve, 800))
      } else {
        await signIn(page)
        await page.evaluate(() => document.fonts.ready.then(() => undefined))
        if (frame.offline === true) {
          // The session exists, so the API can go away. The steps then walk *away* from
          // Today and back, because the screen already mounted while online and a
          // client-side route change is what makes it fetch again — and it is the
          // aborted fetch, not the navigation, that produces the offline state.
          state.offline = true
        }
        if (frame.url !== undefined) {
          // `/styleguide` is a developer gallery with no link into it from the shell,
          // so there is no control to tap. It is the only frame in this table reached
          // this way, and the reason is stated rather than implied.
          await page.evaluate((path) => {
            window.history.pushState({}, '', path)
            window.dispatchEvent(new PopStateEvent('popstate'))
          }, frame.url)
          await new Promise((resolve) => setTimeout(resolve, 600))
        } else {
          await openSurface(page, surfaceFor(frame))
        }
      }

      await page.evaluate(() => document.fonts.ready.then(() => undefined))
      // `networkidle0` is what makes a screenshot of a *settled* screen: the screens
      // fetch on mount and a capture taken mid-flight is a skeleton.
      await page
        .waitForNetworkIdle({ idleTime: 500, timeout: 15000 })
        .catch(() => undefined)
      await new Promise((resolve) => setTimeout(resolve, 400))

      const path = join(OUT, `${frame.name}.png`)
      await page.screenshot({ path, fullPage: true })
      const metrics = await page.evaluate(() => ({
        width: document.documentElement.scrollWidth,
        height: document.documentElement.scrollHeight,
      }))
      console.log(
        `${frame.name.padEnd(28)} ${String(frame.width)}x${String(frame.height)} ` +
          `-> ${String(metrics.width)}x${String(metrics.height)} ${path}`,
      )
    } catch (error) {
      console.error(`${frame.name}: FAILED — ${error.message}`)
    }
    await page.close()
  }

  await browser.close()
}

/**
 * Run only when this file *is* the program.
 *
 * `scripts/probe-frame.mjs` imports `FRAMES` from here so that a probe and a screenshot
 * of the same name are guaranteed to be the same screen at the same width in the same
 * theme — a second copy of the table would let the two drift, and then a probe would be
 * reporting numbers for a screen nobody photographed.
 */
const isEntry =
  process.argv[1] !== undefined && import.meta.url === pathToFileURL(process.argv[1]).href
if (isEntry) await main()

export { FRAMES }
