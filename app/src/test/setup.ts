import '@testing-library/jest-dom/vitest'

// Every test renders through the real i18n instance, in English, so a screen's
// assertions are about the same strings a user sees. A component that calls
// `useTranslation` without this would render its keys instead of its copy, and
// the failure would look like a content bug rather than a missing provider.
import i18n from '@/i18n'

await i18n.changeLanguage('en')

/**
 * `scrollIntoView`, which jsdom does not implement.
 *
 * A browser has it, so nothing in the app guards for its absence — and a component
 * that scrolls a list into view (the command palette, whose focus never leaves its
 * input) would throw here instead of rendering. The stub is inert on purpose: a test
 * that cares whether something was scrolled spies on this function, and one that
 * does not should not have its behaviour changed by a real scroll.
 */
Element.prototype.scrollIntoView = () => {}
