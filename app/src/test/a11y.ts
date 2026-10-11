import axe from 'axe-core'
import { expect } from 'vitest'

/**
 * The automated accessibility gate.
 *
 * `axe` is a floor, not a certificate: it catches the machine-checkable part of
 * WCAG 2.2 AA (names, roles, labels, contrast on rendered colours, heading order,
 * target size in some cases) and says nothing about whether the screen makes
 * sense to a screen-reader user. The spec asks for it in CI anyway, because the
 * defects it *does* catch are the ones that are invisible in a browser and
 * obvious to the person who cannot use the screen.
 *
 * Colour-contrast rules are deliberately disabled here and checked by
 * `src/tokens/contrast.test.ts` instead. jsdom does not apply a stylesheet, so
 * axe would either see nothing or see the default black-on-white and pass
 * everything; the token test computes the real ratios from the real values.
 */

/** Rules disabled because jsdom cannot evaluate them meaningfully. */
const DISABLED: Record<string, { enabled: boolean }> = {
  'color-contrast': { enabled: false },
}

/** Runs axe on a rendered container and fails with a readable list of violations. */
export async function expectNoA11yViolations(container: HTMLElement): Promise<void> {
  const results = await axe.run(container, {
    rules: DISABLED,
    // The component gallery and key screens only; axe on a whole page in jsdom
    // spends most of its time on landmark rules that a component cannot satisfy
    // in isolation.
    resultTypes: ['violations'],
  })

  const report = results.violations.map((violation) => ({
    id: violation.id,
    impact: violation.impact,
    help: violation.help,
    nodes: violation.nodes.map((node) => node.target.join(' ')),
  }))

  expect(report, `${report.length} accessibility violation(s)`).toEqual([])
}
