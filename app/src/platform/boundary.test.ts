import { readdirSync, readFileSync, statSync } from 'node:fs'
import { join } from 'node:path'
import { describe, expect, it } from 'vitest'

const SRC = join(import.meta.dirname, '..')

function walk(dir: string): string[] {
  return readdirSync(dir).flatMap((entry) => {
    const path = join(dir, entry)
    return statSync(path).isDirectory() ? walk(path) : [path]
  })
}

describe('platform boundary', () => {
  it('only the platform adapter imports @tauri-apps packages', () => {
    const offenders = walk(SRC)
      .filter((path) => /\.[tj]sx?$/.test(path))
      .filter((path) => !path.includes('/platform/'))
      .filter((path) => !/\.(test|spec)\.tsx?$/.test(path))
      .filter((path) => /from\s+['"]@tauri-apps\//.test(readFileSync(path, 'utf8')))

    expect(offenders).toEqual([])
  })
})
