/// <reference types="vite/client" />

/**
 * The build-time purchase gate.
 *
 * `true` in the web and desktop builds, `false` in the store build
 * (`VITE_PLATFORM=mobile`). It is a `define` in `vite.config.ts`, so it is
 * substituted as a literal before the minifier runs — which is the only reason a
 * branch guarded by it can actually drop its imports. A `platform.kind()` check
 * cannot: it is a run-time condition, so every module the branch names stays in
 * the file (A322's "tree-shaken out of the mobile chunk" was false until this
 * existed).
 *
 * Declared here rather than read from `import.meta.env` because a missing env var
 * is `undefined` and a silently-false gate would ship the purchase path. The
 * declaration makes it a compile-time constant with one meaning in every build.
 */
declare const __PURCHASE_UI__: boolean
