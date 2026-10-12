/// <reference types="vitest/config" />
import { fileURLToPath, URL } from 'node:url'
import { defineConfig, type HtmlTagDescriptor, type Plugin } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Set by the Tauri CLI when running on a physical device.
const host = process.env.TAURI_DEV_HOST
const isDebug = Boolean(process.env.TAURI_ENV_DEBUG)
const isWindows = process.env.TAURI_ENV_PLATFORM === 'windows'

/**
 * The store build, and the one build-time fact that separates it.
 *
 * The store build is the bundle the iOS and Android shells load, with **no
 * purchase path in it at all**. That has to be a build-time constant rather than a
 * runtime `platform.kind()` check, because a runtime condition does not
 * tree-shake — the branch is dead for a phone at run time but every module it
 * names is still in the file (A322 said "tree-shaken out of the mobile chunk", and
 * until this existed that was false: `billingElementFor` read `kind()` at run
 * time, so `MobilePlanScreen`, `PlanUsageScreen`, `BillingScreen`, `PricingScreen`
 * and `CheckoutScreen` were all in the one entry chunk, along with
 * `POST /billing/checkout`).
 *
 * `__PURCHASE_UI__` is a `define`, so it is substituted as a literal before the
 * minifier runs and `if (__PURCHASE_UI__)` becomes `if (false)`. The imported
 * bindings inside that branch then have no remaining reference, so the modules
 * behind them — and their transitive dependencies, which is where the checkout
 * route and the `POST` live — are dropped rather than merely unreachable.
 *
 * **Which builds are store builds is decided by the shell, not by a convention
 * here.** `tauri build` runs `beforeBuildCommand` with `TAURI_ENV_PLATFORM` set to
 * the target it is packaging for — the Tauri CLI's own `config.schema.json`
 * documents that for `beforeBuildCommand`, and this file already reads the same
 * variable for `windows`. So the shell's one `frontendDist` (`../app/dist`,
 * `tauri.conf.json`) holds a *phone* bundle exactly when the shell is packaging
 * for a phone, and the bytes a store receives are the bytes this build produced.
 * That link is the whole point: a store build written somewhere the shell does not
 * package is an artifact no phone installs, and a check that reads it would be
 * green about the wrong bytes — which is the defect this replaces.
 *
 * `VITE_PLATFORM=mobile` is the same build made without the shell, into
 * `dist-mobile`, so a runner with no Xcode can build and inspect it. It is the
 * fallback rather than the mechanism, and it loses to the shell: if the shell is
 * building for `darwin` and something exported `VITE_PLATFORM=mobile`, the desktop
 * package would otherwise ship a bundle with no purchase UI in it.
 */
const STORE_PLATFORMS = ['ios', 'android']

/** The platform `tauri build` is packaging for, or `undefined` when it is not. */
const shellPlatform = process.env.TAURI_ENV_PLATFORM

/** The store build without the shell — `dist-mobile`, so `dist` is left alone. */
const manualStoreBuild = shellPlatform === undefined && process.env.VITE_PLATFORM === 'mobile'

const isStoreBuild = STORE_PLATFORMS.includes(shellPlatform ?? '') || manualStoreBuild

/**
 * What this build is, written into the artifact itself.
 *
 * The check has to know which build it is reading, and the only answer that cannot
 * be stale is the one inside the artifact: a directory name is a convention, and an
 * env var is the caller's own claim about a build it may not have made. Vite
 * regenerates `index.html` on every build, so the stamp cannot be inherited from an
 * earlier one — which is what makes `store` and `full` trustworthy where a leftover
 * `dist-mobile` would not be.
 *
 * Two values, because there are two bundles: `store` ships to both stores (the
 * bytes are identical — the platform difference is `platform.kind()` at run time),
 * and `full` is the web/desktop one, where a checkout route is legitimate.
 */
function buildVariantStamp(variant: string): Plugin {
  return {
    name: 'stvdy:build-variant',
    transformIndexHtml: (): HtmlTagDescriptor[] => [
      {
        tag: 'meta',
        attrs: { name: 'stvdy-build-target', content: variant },
        injectTo: 'head',
      },
    ],
  }
}

export default defineConfig({
  plugins: [react(), tailwindcss(), buildVariantStamp(isStoreBuild ? 'store' : 'full')],

  // The build-time constant the purchase gate reads. A `define` rather than an
  // env var read at run time, because the whole point is that the minifier can
  // prove the branch dead (see the comment above `isStoreBuild`).
  define: {
    __PURCHASE_UI__: JSON.stringify(!isStoreBuild),
  },

  resolve: {
    alias: {
      '@': fileURLToPath(new URL('./src', import.meta.url)),
      // The repo's shared config, so the app reads the same
      // `billing_policy.toml` the server does rather than a copy that can
      // drift. Imported as text (`?raw`) and parsed in `features/billing`.
      '@config': fileURLToPath(new URL('../config', import.meta.url)),
    },
  },

  // Tauri prints Rust compiler output to the same terminal; never wipe it.
  clearScreen: false,

  // Vite's envPrefix is a startsWith check, so 'TAURI_ENV_*' would never match
  // TAURI_ENV_PLATFORM. Use the bare prefix.
  envPrefix: ['VITE_', 'TAURI_ENV_'],

  server: {
    port: 5173, // must equal build.devUrl in src-tauri/tauri.conf.json
    strictPort: true,
    host: host || false,
    // Vite 8: server.hmr's ws fields are deprecated in favour of server.ws.
    ws: host ? { protocol: 'ws', host, port: 1421 } : undefined,
    watch: { ignored: ['**/src-tauri/**'] },
    // `@config` resolves outside the app root, and the dev server refuses to
    // serve a file outside its allowed list. Granting exactly the one directory
    // keeps the rest of the repo unreachable from the browser.
    fs: { allow: ['.', '../config'] },
  },

  build: {
    // A store build the *shell* asked for writes `dist`, because that is the
    // directory `tauri.conf.json`'s `frontendDist` packages: the bytes a store
    // receives and the bytes `check-no-purchase-ui.mjs` reads have to be the same
    // ones, and keying on `TAURI_ENV_PLATFORM` is what makes them so. The same
    // build made without the shell writes `dist-mobile`, so it cannot destroy the
    // web/desktop build the shell and `check-no-cdn.mjs` both read.
    outDir: manualStoreBuild ? 'dist-mobile' : 'dist',
    // Tailwind 4 requires Safari 16.4+, which is the honest floor for the
    // macOS/iOS WKWebView; Windows/Android use Chromium.
    target: isWindows ? 'chrome111' : 'safari16.4',
    minify: isDebug ? false : 'oxc',
    sourcemap: isDebug,
  },

  test: {
    environment: 'jsdom',
    globals: true,
    setupFiles: ['./src/test/setup.ts'],
    include: ['src/**/*.{test,spec}.{ts,tsx}'],
  },
})
