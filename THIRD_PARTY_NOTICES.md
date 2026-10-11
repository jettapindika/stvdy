# Third-party notices

Stvdy is proprietary software (see `LICENSE`). It bundles and links third-party
components under their own licences, listed here. Nothing in this file grants
rights to Stvdy itself.

## Bundled assets

| Asset | Licence | Notes |
|---|---|---|
| IBM Plex Sans, IBM Plex Mono | SIL Open Font License 1.1 | Self-hosted in `app/public/fonts`, generated from the Google Fonts CSS for Plex Sans 400/500/600 and Plex Mono 400/500, latin and latin-ext subsets. The OFL permits bundling and redistribution, including in a commercial product, provided the fonts are not sold on their own and their licence is included. `app/src/fonts.css` records the same. |
| KaTeX (CSS, JS, fonts) | MIT | Used by `templates/cheatsheet.v1.html` for print math. Fetched from jsdelivr, or from a local mount when the server renders a PDF. |

## Direct dependencies

Rust, from `crates/*/Cargo.toml`:

| Crate | Licence |
|---|---|
| axum, axum-extra, tower, tower-http, tower_governor | MIT |
| tokio, sqlx, tracing, tracing-subscriber | MIT |
| serde, serde_json | MIT OR Apache-2.0 |
| chrono, chrono-tz | MIT OR Apache-2.0 |
| uuid | Apache-2.0 OR MIT |
| thiserror, anyhow | MIT OR Apache-2.0 |
| toml, dotenvy, hex, base64 | MIT OR Apache-2.0 |
| pgvector | MIT |
| secrecy | MIT OR Apache-2.0 |
| keyring-core | MIT OR Apache-2.0 |
| apple-native-keyring-store | MIT OR Apache-2.0 |
| windows-native-keyring-store | MIT OR Apache-2.0 |
| android-native-keyring-store | MIT OR Apache-2.0 |

Tauri and its plugins (`tauri`, `tauri-plugin-*`) are MIT OR Apache-2.0.

Transitive, reached through `url`'s IDNA/ICU4X chain (18 crates, including
`icu_collections`, `icu_locale_core`, `icu_normalizer`, `icu_properties`,
`icu_provider`, `litemap`, `tinystr`, `writeable`, `yoke`):

| Crate group | Licence |
|---|---|
| ICU4X components behind `url` | Unicode-3.0 |

`Unicode-3.0` is permissive and easy to miss precisely because it is not
copyleft, so it never surfaces in a copyleft audit. It still requires the
licence and copyright notice to accompany a distribution.

JavaScript, from `app/package.json`:

| Package | Licence |
|---|---|
| react, react-dom | MIT |
| react-router | MIT |
| @tanstack/react-query | MIT |
| @tauri-apps/api, @tauri-apps/cli, @tauri-apps/plugin-* | MIT OR Apache-2.0 |
| eventsource-parser | MIT |
| vite, @vitejs/plugin-react, vitest | MIT |
| tailwindcss, @tailwindcss/vite | MIT |
| typescript | Apache-2.0 |
| jsdom | MIT |
| @testing-library/* | MIT |

## Copyleft audit

Checked across the full resolved dependency graph, not just direct dependencies.
Exactly one crate offers a copyleft option and it is a disjunction:

- `r-efi` — `MIT OR Apache-2.0 OR LGPL-2.1-or-later`. Stvdy elects **MIT**, so no
  LGPL obligation attaches.

No dependency is GPL, AGPL, LGPL-only, SSPL, BUSL, or non-commercial. The
`proprietary` entries that appear in a licence scan are Stvdy's own crates.

## Regenerating this list

```bash
cargo metadata --format-version 1 \
  | python3 -c "import json,sys; d=json.load(sys.stdin); \
      [print(p['name'], p['version'], p.get('license')) for p in d['packages']]"
npm --prefix app ls --all --json | head
```

Re-check the copyleft audit whenever a dependency is added or a version bumped:
the audit is a claim about the resolved graph, and the resolved graph changes
without the manifests changing.

## Obligation before binaries ship (Phase 15)

This file **names** licences; it does not yet reproduce their texts. Every
permissive licence listed above (MIT, Apache-2.0, the SIL OFL, Unicode-3.0)
requires its copyright notice and permission text to accompany a binary
distribution, and naming the licence is not the same as including it. That
obligation is not discharged by this file as it stands.

No binaries are distributed yet, which is why this is a gate rather than a
defect. Before the first installer or store build, generate the full texts:

```bash
cargo install cargo-about
cargo about generate about.hbs > THIRD_PARTY_LICENSES.md   # Rust side
npx license-checker --production --json > /tmp/npm-licences.json  # npm side
```

and ship the result alongside the installer. Do this in the same change that
adds code signing, so the two cannot drift apart.
