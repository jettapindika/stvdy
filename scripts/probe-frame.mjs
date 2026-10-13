#!/usr/bin/env node
/**
 * Dumps the computed geometry and type of a screen, for comparison against a Figma frame.
 *
 * **Why numbers rather than a diff of two images.** A screenshot tells you *that* two
 * screens differ; it does not tell you that a heading is 20px where the frame says 24,
 * or that a card's radius is 12 where the frame says 8. The frames' own values are
 * exact — `get_metadata` returns every node's `x`, `y`, `width`, `height` — so the
 * comparison that can actually be acted on is Figma's number against the browser's
 * `getComputedStyle`, and that is what this prints.
 *
 * Usage:
 *
 *   node scripts/probe-frame.mjs today-desktop            # the shell + every text node
 *   node scripts/probe-frame.mjs today-desktop --shell    # only the shell constants
 *
 * It shares `screenshot-frames.mjs`'s frame table, so a probe and a screenshot of the
 * same name are always the same screen at the same width in the same theme.
 */
import { FRAMES } from './screenshot-frames.mjs'
import {
  APP_URL,
  findChrome,
  openSurface,
  puppeteer,
  routeFor,
  signIn,
} from './measure-mobile.mjs'

/** The frame named on the command line. */
const name = process.argv[2]
if (name === undefined) {
  console.error('usage: node scripts/probe-frame.mjs <frame-name> [--shell] [--selector=…]')
  process.exit(1)
}
const frame = FRAMES.find((entry) => entry.name === name)
if (frame === undefined) {
  console.error(`no frame named ${name}`)
  process.exit(1)
}
const shellOnly = process.argv.includes('--shell')
const selectorArg = process.argv.find((arg) => arg.startsWith('--selector='))
const selector = selectorArg?.slice('--selector='.length)

const SURFACES = new Map()
for (const surface of (await import('./measure-mobile.mjs')).SURFACES) {
  SURFACES.set(surface.name, surface)
}

/**
 * The element's box, its border and its radius — the three things a frame states
 * exactly and the three a screenshot cannot measure.
 */
const BOX = `(el) => {
  const r = el.getBoundingClientRect()
  const s = getComputedStyle(el)
  return {
    tag: el.tagName.toLowerCase(),
    cls: (el.className && typeof el.className === 'string' ? el.className : '').slice(0, 70),
    x: Math.round(r.x * 10) / 10,
    y: Math.round(r.y * 10) / 10,
    w: Math.round(r.width * 10) / 10,
    h: Math.round(r.height * 10) / 10,
    radius: s.borderRadius,
    border: s.borderTopWidth + ' ' + s.borderTopColor,
    bg: s.backgroundColor,
    pad: [s.paddingTop, s.paddingRight, s.paddingBottom, s.paddingLeft].join(' '),
    gap: s.rowGap + '/' + s.columnGap,
    display: s.display,
  }
}`

const TEXT = `(el) => {
  const r = el.getBoundingClientRect()
  const s = getComputedStyle(el)
  return {
    text: (el.textContent || '').trim().slice(0, 58),
    cls: (el.className && typeof el.className === 'string' ? el.className : '').slice(0, 44),
    x: Math.round(r.x), y: Math.round(r.y),
    w: Math.round(r.width), h: Math.round(r.height),
    font: s.fontFamily.split(',')[0].replace(/"/g, ''),
    size: s.fontSize, lh: s.lineHeight, weight: s.fontWeight,
    ls: s.letterSpacing, color: s.color,
    transform: s.textTransform,
  }
}`

async function main() {
  const chrome = findChrome()
  if (chrome === null) {
    console.error('no Chromium found; set CHROME_PATH')
    process.exit(1)
  }
  const browser = await puppeteer.launch({
    executablePath: chrome,
    headless: true,
    args: ['--no-sandbox', '--disable-dev-shm-usage'],
  })
  const page = await browser.newPage()
  await page.setViewport({ width: frame.width, height: 900, deviceScaleFactor: 1 })
  await page.emulateMediaFeatures([{ name: 'prefers-color-scheme', value: frame.theme ?? 'light' }])
  await page.setRequestInterception(true)
  page.on('request', (request) => {
    const url = new URL(request.url())
    if (url.origin === new URL(APP_URL).origin) return request.continue()
    const headers = {
      'access-control-allow-origin': '*',
      'access-control-allow-headers': '*',
      'access-control-allow-methods': '*',
      'content-type': 'application/json',
    }
    if (request.method() === 'OPTIONS') {
      return request.respond({ status: 204, headers, body: '' })
    }
    const handler = routeFor(url.pathname)
    return request.respond({
      status: handler === undefined ? 404 : 200,
      headers,
      body: JSON.stringify(handler === undefined ? { code: 'not_found' } : handler()),
    })
  })

  await page.goto(APP_URL, { waitUntil: 'domcontentloaded' })
  if (frame.preAuth !== undefined) {
    await page.goto(`${APP_URL}${frame.preAuth}`, { waitUntil: 'domcontentloaded' })
  } else {
    await signIn(page)
    if (frame.url !== undefined) {
      await page.goto(`${APP_URL}${frame.url}`, { waitUntil: 'domcontentloaded' })
      await new Promise((resolve) => setTimeout(resolve, 600))
    } else {
      const surface = SURFACES.get(frame.surface)
      if (surface === undefined) {
        console.error(`no surface named ${String(frame.surface)}`)
        process.exit(1)
      }
      await openSurface(page, surface)
    }
  }
  await page.evaluate(() => document.fonts.ready.then(() => undefined))
  await new Promise((resolve) => setTimeout(resolve, 400))

  console.log(`### ${name} — figma ${frame.figma} @ ${String(frame.width)}x${String(frame.height)} ${frame.theme ?? 'light'}`)

  // The shell's own constants, which the design states exactly: the rail, the bar, the
  // page's inline padding. These are the numbers every screen shares.
  const shell = await page.evaluate((boxFn) => {
    const box = eval(`(${boxFn})`)
    const out = {}
    const pick = (key, el) => {
      if (el !== null) out[key] = box(el)
    }
    pick('aside', document.querySelector('aside'))
    pick('header', document.querySelector('header'))
    pick('main', document.querySelector('main'))
    pick('h1', document.querySelector('h1'))
    pick('nav', document.querySelector('nav'))
    // The first nav link, which is the item the design draws at 176x44.
    pick('nav-item', document.querySelector('nav a'))
    return out
  }, BOX)
  for (const [key, value] of Object.entries(shell)) {
    console.log(
      `  ${key.padEnd(10)} ${String(value.w).padStart(7)}x${String(value.h).padEnd(7)} ` +
        `at ${String(value.x)},${String(value.y)}  r=${value.radius} pad=${value.pad} bg=${value.bg}`,
    )
  }

  if (shellOnly) {
    await browser.close()
    return
  }

  if (selector !== undefined) {
    const boxes = await page.evaluate(
      (sel, boxFn) => {
        const box = eval(`(${boxFn})`)
        return [...document.querySelectorAll(sel)].map(box)
      },
      selector,
      BOX,
    )
    console.log(`\n### ${selector} (${String(boxes.length)})`)
    for (const b of boxes) {
      console.log(
        `  ${b.tag}.${b.cls.slice(0, 40).padEnd(40)} ${String(b.w).padStart(7)}x${String(b.h).padEnd(7)} ` +
          `at ${String(b.x)},${String(b.y)} r=${b.radius} pad=${b.pad}`,
      )
    }
  }

  const texts = await page.evaluate(
    (textFn) => {
      const probe = eval(`(${textFn})`)
      const out = []
      const walker = document.createTreeWalker(document.body, NodeFilter.SHOW_TEXT)
      const seen = new Set()
      let node
      while ((node = walker.nextNode()) !== null) {
        const value = (node.textContent ?? '').trim()
        if (value === '') continue
        const parent = node.parentElement
        if (parent === null || seen.has(parent)) continue
        seen.add(parent)
        out.push(probe(parent))
      }
      return out
    },
    TEXT,
  )
  console.log(`\n### text nodes (${String(texts.length)})`)
  for (const t of texts) {
    console.log(
      `  ${t.font.padEnd(18)} ${t.size.padStart(6)}/${t.lh.padEnd(7)} w${t.weight} ls=${t.ls.padEnd(6)} ` +
        `${t.color.padEnd(20)} ${t.transform.padEnd(10)} ${String(t.x).padStart(5)},${String(t.y).padStart(5)} ` +
        `${String(t.w).padStart(5)}x${String(t.h).padStart(4)}  ${JSON.stringify(t.text)}`,
    )
  }

  await browser.close()
}

await main()
