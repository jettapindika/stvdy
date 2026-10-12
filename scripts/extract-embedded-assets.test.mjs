import { execFileSync } from 'node:child_process'
import { mkdtempSync, readFileSync, writeFileSync } from 'node:fs'
import { tmpdir } from 'node:os'
import { join } from 'node:path'
import { brotliCompressSync } from 'node:zlib'
import assert from 'node:assert/strict'
import { test } from 'node:test'

// The extractor exists because a Tauri APK carries its frontend inside the native library
// rather than as archive members, so `check-mobile-no-purchase.sh` could not read the bytes a
// phone installs and failed on every real APK. Its contract is "recover the bundle, or fail"
// — and the second half is the one that matters, because a scanner that recovers nothing and
// passes anyway is the defect this whole gate exists to prevent.
//
// The fixture is a synthetic library rather than a real `.so`: `rust-embed` 8.13.0's layout is
// `[name][brotli stream]` concatenated, which is all the extractor reads, and a real 11 MB
// library in the test suite would be neither deterministic nor reviewable.

const extractor = new URL('./extract-embedded-assets.mjs', import.meta.url).pathname

/** A library body holding `[name][brotli]` for each entry, as `rust-embed` writes it. */
function library(entries) {
  const parts = [Buffer.from('junk-prefix')]
  for (const [name, body] of entries) {
    parts.push(Buffer.from(name), brotliCompressSync(Buffer.from(body)))
  }
  return Buffer.concat(parts)
}

function run(body) {
  const dir = mkdtempSync(join(tmpdir(), 'embedded-assets-'))
  const lib = join(dir, 'libstvdy_shell_lib.so')
  writeFileSync(lib, body)
  const out = join(dir, 'out')
  try {
    const stdout = execFileSync('node', [extractor, lib, out], { encoding: 'utf8' })
    return { status: 0, stdout, out }
  } catch (error) {
    return { status: error.status, stderr: error.stderr, out }
  }
}

const HTML = '<meta name="stvdy-build-target" content="store">'
const JS = 'console.log("entry")'

test('recovers the bundle a Tauri library embeds', () => {
  const result = run(
    library([
      ['index.html', HTML],
      ['assets/index-abc123.js', JS],
    ]),
  )

  assert.equal(result.status, 0)
  assert.match(result.stdout, /recovered 2 embedded file\(s\)/)
  assert.equal(readFileSync(join(result.out, 'index.html'), 'utf8'), HTML)
  assert.equal(readFileSync(join(result.out, 'assets/index-abc123.js'), 'utf8'), JS)
})

test('fails when the library embeds no bundle at all', () => {
  const result = run(Buffer.from('a library with nothing embedded'))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /no web assets recovered/)
})

test('fails when the bundle is missing its entry script', () => {
  // An `index.html` alone is a bundle whose stamp can be read and whose code cannot — a scan
  // of it would report a clean artifact on evidence it does not have.
  const result = run(library([['index.html', HTML]]))

  assert.equal(result.status, 1)
  assert.match(result.stderr, /no \.js/)
})

test('refuses a bundle with no stamp to read', () => {
  // The stamp is the shared check's identity gate, so an unstamped index.html has to reach it
  // rather than be filtered here: this test pins that the extractor does not decide identity.
  const result = run(
    library([
      ['index.html', '<html></html>'],
      ['assets/index-abc123.js', JS],
    ]),
  )

  assert.equal(result.status, 0)
  assert.equal(readFileSync(join(result.out, 'index.html'), 'utf8'), '<html></html>')
})
