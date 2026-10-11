import '@testing-library/jest-dom/vitest'

// Every test renders through the real i18n instance, in English, so a screen's
// assertions are about the same strings a user sees. A component that calls
// `useTranslation` without this would render its keys instead of its copy, and
// the failure would look like a content bug rather than a missing provider.
import i18n from '@/i18n'

await i18n.changeLanguage('en')
