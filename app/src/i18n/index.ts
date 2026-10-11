import i18n from 'i18next'
import { initReactI18next } from 'react-i18next'

import en from './locales/en.json'
import id from './locales/id.json'
import { PSEUDO_LANGUAGE, pseudoResource } from './pseudo'

/**
 * Languages the app ships.
 *
 * English and Bahasa Indonesia from day one, per the spec's § i18n: the audience
 * is Indonesian students, but the source material and the technical vocabulary
 * are frequently English, so neither language is a translation of the other's
 * afterthought.
 */
export const LANGUAGES = ['en', 'id'] as const
export type Language = (typeof LANGUAGES)[number]

/** The language names as the settings screen shows them, each in its own language. */
export const LANGUAGE_LABELS: Record<Language, string> = {
  en: 'English',
  id: 'Bahasa Indonesia',
}

/** True when `value` is a language the app ships. */
export function isLanguage(value: string): value is Language {
  return (LANGUAGES as readonly string[]).includes(value)
}

/**
 * The language to start in.
 *
 * The OS or browser language decides, because a student who has set their phone
 * to Indonesian should not have to find a setting before the app is usable. A
 * regional variant (`id-ID`, `en-GB`) resolves to its base language; anything
 * else falls back to English rather than to a machine-translated guess.
 */
export function detectLanguage(): Language {
  const candidates = [
    ...(typeof navigator === 'undefined' ? [] : navigator.languages ?? [navigator.language]),
  ]
  for (const candidate of candidates) {
    const base = candidate.split('-')[0]?.toLowerCase() ?? ''
    if (isLanguage(base)) return base
  }
  return 'en'
}

/** The date/time/currency locale for a language. `id` formats as `Rp 10.000`. */
export const INTL_LOCALE: Record<Language, string> = {
  en: 'en-GB',
  id: 'id-ID',
}

/** Whether the language uses a 24-hour clock by default. Indonesian does. */
export const HOUR_CYCLE: Record<Language, 'h12' | 'h23'> = {
  en: 'h12',
  id: 'h23',
}

void i18n.use(initReactI18next).init({
  resources: {
    en: { translation: en },
    id: { translation: id },
  },
  lng: detectLanguage(),
  fallbackLng: 'en',
  // ICU-style plurals and interpolation, per the spec. `_one`/`_other` suffixes
  // in the locale files, so a count that reads naturally in English also reads
  // naturally in Indonesian (which has no plural form).
  interpolation: { escapeValue: false },
  returnNull: false,
})

// The pseudo-locale is registered only in a dev build. It exists so a layout can
// be seen with strings ~30% longer than English, which is what Indonesian looks
// like and what a screenshot check measures; shipping it would add a language
// nobody can select.
if (import.meta.env.DEV) {
  i18n.addResourceBundle(PSEUDO_LANGUAGE, 'translation', pseudoResource(en), true, true)
}

export default i18n
