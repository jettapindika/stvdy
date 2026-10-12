# The print assets

The cheatsheet template (`templates/cheatsheet.v1.html`) needs KaTeX and two typefaces.
Until Phase 15 it loaded all three from third-party origins — `cdn.jsdelivr.net` and
`fonts.googleapis.com` — so a printed document depended on those being reachable. A blocked
or slow CDN does not fail the render: it leaves every formula as literal `$…$` and the text
in a fallback face, and the PDF is produced anyway. That is the silent defect
`crates/server/tests/pdf.rs::the_cheatsheet_renders_to_a_six_page_a4_pdf` exists to catch, so
the bytes live here instead.

Three things keep the bytes here and honest:

- `scripts/fetch-print-assets.sh` downloads them and verifies each one against
  `checksums.txt` **before** writing it. A CDN that changed its content fails the script
  rather than silently swapping a shipped asset for a different one.
- `crates/server/src/assets.rs` embeds this directory with `rust-embed` at compile time, so
  a missing file is a build failure rather than a 404 at print time. The server serves it
  from `GET /assets/{*path}`.
- `crates/server/src/render/chrome.rs::rewrite_asset_base` points the template's CDN
  references at that route before the page is written into Chromium.

Regenerating after a version bump is a deliberate two-step, because a checksum list that
updates itself verifies nothing: edit the URLs in `checksums.txt`, run
`bash scripts/fetch-print-assets.sh --update`, then read the hash diff — a changed hash is a
changed asset, and that diff is the review.

## What is here

| Path | Version | Source | Notes |
|---|---|---|---|
| `katex/katex.min.css` | 0.19.0 | `https://cdn.jsdelivr.net/npm/katex@0.19.0/dist/katex.min.css` | the version the template's CDN URLs name |
| `katex/katex.min.js` | 0.19.0 | `…/dist/katex.min.js` | `defer`red by the template |
| `katex/contrib/auto-render.min.js` | 0.19.0 | `…/dist/contrib/auto-render.min.js` | what `renderMathInElement` comes from |
| `katex/fonts/*.woff2` | 0.19.0 | `…/dist/fonts/*.woff2` | the 20 faces `katex.min.css` names, all of them: a subset would leave some symbol as a fallback glyph |
| `fonts.css` | generated | Google Fonts, `fonts.gstatic.com` | the replacement for the `fonts.googleapis.com` stylesheet; only `latin` and `latin-ext` |
| `fonts/inter-*.woff2` | v20 | `fonts.gstatic.com/s/inter/v20/…` | 400/600/700, the weights the template names |
| `fonts/jetbrains-mono-*.woff2` | v24 | `fonts.gstatic.com/s/jetbrainsmono/v24/…` | 400/600, for code and page references |
| `checksums.txt` | — | — | the pinned SHA-256 per file; the script's authority |

`latin-ext` is not optional. Indonesian source material and romanised names carry
characters outside basic latin, and dropping the subset renders them mid-sentence in a
fallback face.

The four font files are byte-identical to the SPA's copies in `app/public/fonts/`
(`scripts/fetch-fonts.mjs` fetched the same content-addressed URLs for the same families),
which is why their names carry the same hash suffix. They are separate copies on purpose:
the SPA's are served from the web build's own root and the print assets from
`/assets`, and one file that two build systems rewrite is a file that drifts.

Licences: KaTeX is MIT; Inter and JetBrains Mono are SIL Open Font License 1.1.
`THIRD_PARTY_NOTICES.md` carries the entries.

## Not served

`checksums.txt` and this file are in the directory but not on the HTTP surface: the route
serves only `.css`, `.js` and `.woff2` and answers 404 for anything else. They are here for
the release engineer, and a build record has no business being a public URL.
