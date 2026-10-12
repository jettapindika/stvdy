/**
 * The syllabus review screen's rules, as pure functions.
 *
 * **The screen is the phase's honesty test, and its two rules are about what it
 * does not do.** Nothing is written until the student confirms, so this module
 * builds the confirm body and an item the student dropped has no index in it; and
 * the extraction's score is never rendered (A261), so the rows this module returns
 * carry the **evidence quote** — the exact source line the item was read from —
 * and no number.
 *
 * The score's only job on the server is to order the list and to flag an item as
 * needing a check, and both arrive already done: `GET /syllabus/:id/draft` answers
 * a pre-ordered list with a `needs_check` flag and **no score field at all**. So
 * this module **preserves the order it is given** rather than re-sorting. A second
 * ordering rule here would be a second answer to "which item is most likely
 * wrong", and the two would drift the moment the threshold moved.
 *
 * `reviewRows` also builds each row field by field rather than spreading the wire
 * object. That is deliberate: a spread would carry any field the server adds into
 * the render, and the one field that must never reach the screen is the one a
 * future server could start sending again.
 */

/** The three kinds the server names, which are `ReviewKind::as_str`'s values. */
export type ReviewKind = 'class' | 'deadline' | 'exam'

/**
 * One row of the review screen.
 *
 * `evidence` is the substance rather than a field to fill in: it is the source
 * line the item was read from, and it is what lets the student check the item
 * before anything is written. An item with no quote is an item they cannot check.
 */
export interface ReviewItem {
  /** The item's position in the extraction, which is how the confirm names it. */
  index: number
  kind: ReviewKind
  title: string
  evidence: string
  /** True when the extraction was unsure enough to warrant a look (A261). */
  needs_check: boolean
  /** The absolute instant, when the item has one. A meeting has none. */
  when: string | null
  rrule: string | null
  /** `HH:MM`, for a meeting. */
  start_time: string | null
  end_time: string | null
  topics: string[]
  weight_percent: number | null
}

/** One item's editable state, as the screen holds it. */
export interface ReviewDraft {
  index: number
  /** False when the student dropped it. */
  keep: boolean
  title: string
  when: string | null
  rrule: string | null
  topics: string[]
}

const KINDS: readonly ReviewKind[] = ['class', 'deadline', 'exam']

function text(value: unknown, fallback = ''): string {
  return typeof value === 'string' ? value : fallback
}

function optionalText(value: unknown): string | null {
  return typeof value === 'string' && value.length > 0 ? value : null
}

function optionalNumber(value: unknown): number | null {
  return typeof value === 'number' && Number.isFinite(value) ? value : null
}

/**
 * The rows the screen renders, from the body `GET /syllabus/:id/draft` answered.
 *
 * The order is the server's, unchanged. A row the body cannot be read as is
 * dropped rather than rendered half-empty: an item with no index cannot be
 * confirmed, and one with no evidence quote is an item the student has no way to
 * check — which is the whole reason the review step exists.
 */
export function reviewRows(raw: unknown): ReviewItem[] {
  if (!Array.isArray(raw)) return []

  const rows: ReviewItem[] = []
  for (const entry of raw) {
    if (typeof entry !== 'object' || entry === null) continue
    const item = entry as Record<string, unknown>
    const index = item['index']
    const kind = item['kind']
    if (typeof index !== 'number' || !KINDS.includes(kind as ReviewKind)) continue

    const evidence = text(item['evidence']).trim()
    if (evidence.length === 0) continue

    rows.push({
      index,
      kind: kind as ReviewKind,
      title: text(item['title']),
      evidence,
      needs_check: item['needs_check'] === true,
      when: optionalText(item['when']),
      rrule: optionalText(item['rrule']),
      start_time: optionalText(item['start_time']),
      end_time: optionalText(item['end_time']),
      topics: Array.isArray(item['topics'])
        ? item['topics'].filter((topic): topic is string => typeof topic === 'string')
        : [],
      weight_percent: optionalNumber(item['weight_percent']),
    })
  }
  return rows
}

/** The screen's initial state: every item kept, with the extraction's own values. */
export function draftFrom(items: ReviewItem[]): ReviewDraft[] {
  return items.map((item) => ({
    index: item.index,
    keep: true,
    title: item.title,
    when: item.when,
    rrule: item.rrule,
    topics: item.topics,
  }))
}

/** How many items the student has left marked as needing a look. */
export function checkCount(items: ReviewItem[]): number {
  return items.filter((item) => item.needs_check).length
}

/** How many items the confirm will write rows for. */
export function keptCount(drafts: ReviewDraft[]): number {
  return drafts.filter((draft) => draft.keep).length
}

/**
 * One item of the confirm body, in the shape `POST /syllabus/:id/confirm` reads.
 *
 * Every field but `index` and `keep` is optional and means "keep what the
 * extraction said" rather than "clear it", so only the student's edits are sent.
 * Sending the extraction's own value back for an untouched item would be
 * indistinguishable from an edit, and a `null` where the student cleared a title
 * would blank an item they accepted.
 */
export interface ConfirmItem {
  index: number
  keep: boolean
  title?: string
  when?: string
  rrule?: string
  topics?: string[]
}

/**
 * The confirm body.
 *
 * **A dropped item has no index in it.** The server refuses a body naming an index
 * that is not in the draft and writes only the items marked `keep`, so sending a
 * dropped item would be noise at best — but the rule is stated here rather than
 * left to the server, because "the student dropped it" and "the payload did not
 * mention it" must be the same fact, and a body that carried a dropped item's
 * edits would be a body that could write them.
 */
export function confirmBody(drafts: ReviewDraft[], items: ReviewItem[]): { items: ConfirmItem[] } {
  const source = new Map(items.map((item) => [item.index, item]))
  const body: ConfirmItem[] = []

  for (const draft of drafts) {
    if (!draft.keep) continue
    const item = source.get(draft.index)
    if (item === undefined) continue

    const sent: ConfirmItem = { index: draft.index, keep: true }
    if (draft.title !== item.title) sent.title = draft.title
    // A cleared date is not expressible: the server reads an absent `when` as "keep
    // what the extraction said", so the screen does not offer clearing one and a
    // `null` here means no edit rather than a blank.
    if (draft.when !== item.when && draft.when !== null) sent.when = draft.when
    if (draft.rrule !== item.rrule && draft.rrule !== null) sent.rrule = draft.rrule
    if (draft.topics.join('\n') !== item.topics.join('\n')) sent.topics = draft.topics
    body.push(sent)
  }

  return { items: body }
}
