import type { SyncApi } from '@/platform'

/**
 * A `Platform.sync.read` served from rows a test holds.
 *
 * The Today view and the Tasks screen read synced entities from the device's
 * cache, and the point of most of those tests is that the screen works with the
 * **network down**. So the double is not a mock that records calls: it is a local
 * store the screen reads, with `fetch` failing, which is what a device with a
 * cache and no signal actually is (A14/A205).
 *
 * Both of `SyncApi.read`'s shapes are served from the one map, because the real
 * shell command is the same command: a device that is offline has to enumerate
 * today's rows as well as fetch one by id.
 */

export function fakeCache(rows: Record<string, unknown[]>): {
  read: SyncApi['read']
  /** Replaces one entity's rows, as a pull applying a page would. */
  set(entity: string, next: unknown[]): void
} {
  const store: Record<string, unknown[]> = { ...rows }

  const rowsOf = (entity: string): unknown[] => store[entity] ?? []

  // A cache row always carries an id, and this is the guard that says so rather
  // than an assertion: a row without one simply never matches a single-row read.
  const idOf = (row: unknown): string | null => {
    if (row === null || typeof row !== 'object' || !('id' in row)) return null
    const value = row.id
    return typeof value === 'string' ? value : null
  }

  // The overloaded signature has no implementation TypeScript can infer through a
  // generic arrow, so the cast names the contract the three branches above meet.
  const read = (async <T,>(entity: string, id?: string): Promise<T[] | T | null> => {
    const found = rowsOf(entity)
    if (id === undefined) {
      // `unknown[]` → the caller's row type, because `read<T>` *is* the caller's
      // statement about which entity's shape it asked this store for.
      const all = found as T[]
      return all
    }
    // Same reasoning for the single row: the id guard above is what picked it.
    const one = found.find((row) => idOf(row) === id) ?? null
    return one as T | null
  }) as SyncApi['read']

  return {
    read,
    set(entity, next) {
      store[entity] = next
    },
  }
}
