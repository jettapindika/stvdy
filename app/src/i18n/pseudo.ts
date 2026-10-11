/**
 * The pseudo-locale: English with every string stretched ~30% longer.
 *
 * The point is not translation. Indonesian runs longer than English, and a
 * layout that fits "Add source" can still clip "Tambahkan sumber"; a pseudo-locale
 * makes that visible without waiting for the real translation, and it makes it
 * visible in a screenshot test where a jsdom test cannot measure anything.
 *
 * The accent substitution also catches the opposite defect: text that is
 * hard-coded in a component does not change under the pseudo-locale, so a screen
 * that is *mostly* accented has an unextracted literal in it.
 */

/** Accented stand-ins that keep the letter's shape, so a word stays readable. */
const ACCENTS: Record<string, string> = {
  a: 'á',
  e: 'é',
  i: 'í',
  o: 'ó',
  u: 'ú',
  c: 'ç',
  n: 'ñ',
  s: 'š',
  y: 'ý',
  A: 'Á',
  E: 'É',
  I: 'Í',
  O: 'Ó',
  U: 'Ú',
  C: 'Ç',
  N: 'Ñ',
  S: 'Š',
  Y: 'Ý',
}

/** How much longer the pseudo-locale makes a string. */
export const EXPANSION = 1.3

/**
 * Stretches one string to at least `EXPANSION` times its length.
 *
 * `{{placeholders}}` are copied through untouched. Accenting a placeholder's
 * letters (`{{count}}` → `{{çóúñt}}`) would stop i18next resolving it, and the
 * pseudo-locale render would show a literal `{{çóúñt}}` — which reads as an app
 * bug rather than the layout problem this exists to expose. The length guarantee
 * is applied to the surrounding text instead, which is what the check is about.
 *
 * Vowels are doubled rather than a character repeated verbatim, so the result
 * still looks like a word: a run of the same character reads as a rendering bug
 * and gets ignored in review, which defeats the purpose. Words with no vowel grow
 * by repeating themselves, because the guarantee matters more than the look.
 */
export function stretch(value: string): string {
  // Split so each placeholder is its own token, and every other token is text.
  return value
    .split(/(\{\{[^}]*\}\})/)
    .map((part) => (part.startsWith('{{') && part.endsWith('}}') ? part : stretchText(part)))
    .join('')
}

/** The text half of `stretch`: no placeholder handling. */
function stretchText(value: string): string {
  const words = value.split(/(\s+)/)
  const grown = words.map((word) => {
    if (/^\s*$/.test(word) || word === '') return word
    const accented = [...word].map((char) => ACCENTS[char] ?? char).join('')
    const target = Math.ceil(word.length * EXPANSION)
    if (accented.length >= target) return accented

    const vowelAt = [...accented]
      .map((char, index) => (isVowel(char) ? index : -1))
      .filter((index) => index >= 0)
    // `repeat` of the whole accented word is the last resort for a word with no
    // vowel, e.g. "km" or "th".
    const filler = vowelAt.length > 0 ? accented : accented.repeat(2)

    let result = accented
    let step = 0
    while (result.length < target) {
      if (vowelAt.length === 0) {
        result += accented
        continue
      }
      // Insert a duplicate of a vowel, walking outward from the middle so the
      // padding is spread through the word rather than piled on the end.
      const middle = Math.floor(vowelAt.length / 2)
      const offset = Math.ceil(step / 2) * (step % 2 === 0 ? 1 : -1)
      const clamped = Math.min(Math.max(middle + offset, 0), vowelAt.length - 1)
      const at = vowelAt[clamped] ?? 0
      const char = filler[at] ?? '\u00b7'
      result = result.slice(0, at + 1) + char + result.slice(at + 1)
      step += 1
    }
    return result
  })
  return grown.join('')
}

function isVowel(char: string): boolean {
  return 'aeiouAEIOUáéíóúÁÉÍÓÚ'.includes(char)
}

/** The pseudo-locale for a resource object, key for key. */
export function pseudoResource(resource: unknown): Record<string, unknown> {
  if (typeof resource === 'string') return { value: stretch(resource) } as never
  if (Array.isArray(resource)) return resource.map(pseudoResource) as never
  if (resource && typeof resource === 'object') {
    const out: Record<string, unknown> = {}
    for (const [key, value] of Object.entries(resource as Record<string, unknown>)) {
      out[key] = typeof value === 'string' ? stretch(value) : pseudoResource(value)
    }
    return out
  }
  return resource as never
}

/** The language tag the pseudo-locale registers under. */
export const PSEUDO_LANGUAGE = 'en-XA'
