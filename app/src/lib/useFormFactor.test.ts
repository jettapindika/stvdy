import { describe, expect, it } from 'vitest'

import { formFactorForWidth } from './useFormFactor'

describe('form factor is a width, not a platform', () => {
  it('maps the three layout buckets at the documented breakpoints', () => {
    expect(formFactorForWidth(390)).toBe('phone')
    expect(formFactorForWidth(767)).toBe('phone')
    // 768 is the `md` boundary the CSS uses for the sidebar/tab-bar swap.
    expect(formFactorForWidth(768)).toBe('tablet')
    expect(formFactorForWidth(1023)).toBe('tablet')
    // 1024 is the `lg` boundary the workspace uses for its three-panel layout.
    expect(formFactorForWidth(1024)).toBe('desktop')
    expect(formFactorForWidth(2560)).toBe('desktop')
  })

  it('keeps the JS boundary identical to the CSS one', () => {
    // These numbers must match the Tailwind prefixes used in Sidebar/TabBar
    // (`md:`) and WorkspaceScreen (`lg:`). If they drift, JS and CSS disagree
    // about which shell to render, which is how the missing-navigation bug
    // happened: CSS hid the sidebar below 640px while JS never rendered the bar.
    expect(formFactorForWidth(767)).toBe('phone')
    expect(formFactorForWidth(768)).toBe('tablet')
    expect(formFactorForWidth(1023)).toBe('tablet')
    expect(formFactorForWidth(1024)).toBe('desktop')
  })
})
