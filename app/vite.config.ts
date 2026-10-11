/// <reference types="vitest/config" />
import { fileURLToPath, URL } from 'node:url'
import { defineConfig } from 'vite'
import react from '@vitejs/plugin-react'
import tailwindcss from '@tailwindcss/vite'

// Set by the Tauri CLI when running on a physical device.
const host = process.env.TAURI_DEV_HOST
const isDebug = Boolean(process.env.TAURI_ENV_DEBUG)
const isWindows = process.env.TAURI_ENV_PLATFORM === 'windows'

export default defineConfig({
  plugins: [react(), tailwindcss()],

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
