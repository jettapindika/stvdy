#!/usr/bin/env node
//
// Extracts the web assets a **Tauri mobile app** ships, so the absence check can read the
// bytes a phone actually installs.
//
// Why this exists. `scripts/check-mobile-no-purchase.sh` reads a directory of built assets,
// and for an `.apk`/`.ipa` it extracts the archive and scans its members. That is correct for
// a plain webview app and **wrong for a Tauri one**: the frontend is not a member of the
// archive. `tauri::generate_context!()` embeds it into the native library through `rust-embed`,
// so an Android APK carries the bundle inside `lib/<abi>/libstvdy_shell_lib.so` and its
// `assets/` directory holds only `tauri.conf.json` and the dexopt profile. Pointed at a real
// APK, the check therefore found no `index.html`, reported "no stvdy-build-target stamp in the
// artifact", and **failed every time** — a gate that could not pass on the artifact it exists
// to gate. Found by building one; the APK in question was stamped `full` and carried six
// `billing/checkout` strings, which is exactly what the gate was supposed to catch.
//
// The format, measured rather than guessed (`rust-embed` 8.13.0): the library concatenates
// `[name][brotli stream]` for each embedded file, so a file's bytes are found by locating its
// name and decompressing from the byte after it. Verified on a real `libstvdy_shell_lib.so`:
// nine assets recovered this way — `index.html`, the entry `.js`, the `.css` and six fonts —
// with the JavaScript decompressing to 775,053 bytes carrying the API origin the build was
// given.
//
// **Fail closed.** An artifact whose assets cannot be recovered is a failure, never a skip:
// this is the same rule the shared check applies to an empty directory, and for the same
// reason — a scan of nothing proves nothing about what a store receives.
//
// Usage: extract-embedded-assets.mjs <library.so> <out-dir>
//
// Exit: 0 assets written (at least index.html and one .js); 1 nothing recoverable.

import { mkdirSync, readFileSync, writeFileSync } from 'node:fs'
import { brotliDecompressSync } from 'node:zlib'
import { dirname, join } from 'node:path'

const [library, outDir] = process.argv.slice(2)
if (!library || !outDir) {
  console.error('usage: extract-embedded-assets.mjs <library.so> <out-dir>')
  process.exit(1)
}

const bytes = readFileSync(library)

// A name is a path ending in an extension this app ships. The library stores the path as it
// appeared relative to `frontendDist`, and a stray byte before it (the previous entry's tail)
// is why the pattern does not anchor at the start.
const NAMES = /[A-Za-z0-9_.@/-]*\.(?:html|js|css|svg|png|woff2?|json)/g
const text = bytes.toString('latin1')

const written = new Map()
for (const match of text.matchAll(NAMES)) {
  const name = match[0]
  const from = match.index + name.length
  let decoded
  try {
    // `brotliDecompressSync` needs an end, and the stream is self-terminating, so the bound
    // is only a guard against a pathological input.
    decoded = brotliDecompressSync(bytes.subarray(from, from + 8_000_000))
  } catch {
    continue
  }
  if (decoded.length === 0) continue

  // Strip whatever preceded the path: the real relative path starts at the first segment
  // after any leading junk, and only the basename matters to the check — it needs
  // `index.html` at the root for the stamp and every other file for the byte scan.
  const clean = name.replace(/^.*?(?=(?:index\.html$)|(?:assets\/)|(?:fonts\/))/s, '')
  if (clean.length === 0) continue
  // First write wins: the same asset appears once per ABI's library, and the scan is per file.
  if (written.has(clean)) continue
  written.set(clean, decoded)
}

const html = [...written.keys()].find((k) => k === 'index.html')
const scripts = [...written.keys()].filter((k) => k.endsWith('.js'))
if (html === undefined || scripts.length === 0) {
  console.error(
    `no web assets recovered from ${library}: found ${written.size} file(s)` +
      `${html === undefined ? ', no index.html' : ''}` +
      `${scripts.length === 0 ? ', no .js' : ''}` +
      ' — an artifact whose bundle cannot be read is not a clean artifact',
  )
  process.exit(1)
}

for (const [name, content] of written) {
  const path = join(outDir, name)
  mkdirSync(dirname(path), { recursive: true })
  writeFileSync(path, content)
}

console.log(`recovered ${written.size} embedded file(s) from ${library}`)
for (const [name, content] of written) console.log(`  ${name} ${content.length} bytes`)
