import { platform } from '@/platform'

/**
 * Which of the two backends this build reads and writes a synced entity through.
 *
 * The SPA has one data layer and two transports. A build with a local cache — the
 * shell — reads and writes through `platform.sync`, which is the cache and the
 * outbox, and is why the Today view and the timer work with the network down. The
 * web build has neither (A14/A183): its `sync.read` answers nothing and its
 * `sync.write` refuses, because accepting a write it has nowhere to queue would
 * mark a row saved that no pass will ever deliver.
 *
 * So the web build uses the REST routes, which are not a second write path: they
 * are the same table triggers the push applies through, and the roadmap says so.
 * What matters is that the *choice* is one function rather than a conditional
 * spread across the screens, so a screen cannot accidentally read from one
 * backend and write to the other.
 *
 * This is read per call rather than once at module scope: the shell installs its
 * adapter during startup, after this module is evaluated, and a constant captured
 * at import time would have every native build take the web branch.
 */
export function hasLocalCache(): boolean {
  return platform.runtime() !== 'web'
}
