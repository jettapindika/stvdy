import { useEffect, useMemo, useState } from 'react'
import { useTranslation } from 'react-i18next'
import { Link } from 'react-router'

import { asApiError, type ApiError } from '@/api/errors'
import { confirmSyllabus, fetchDraft, type DraftBody } from '@/api/queries/syllabus'
import { PageHeader, Section } from '@/components/layout/Section'
import { ApiErrorNotice, EmptyState, LoadingState } from '@/components/ui/States'
import { Icon } from '@/components/ui/Icon'
import { DEFAULT_TIMEZONE, formatInZone } from '@/lib/time'
import { useSession } from '@/features/auth/useSession'
import {
  checkCount,
  confirmBody,
  draftFrom,
  keptCount,
  reviewRows,
  type ReviewDraft,
} from './review'

/**
 * The syllabus review screen: the phase's honesty test.
 *
 * The extraction is a model's reading of a document, and a wrong reading written
 * straight through is the failure this screen exists to prevent. So four things
 * are true here and each is a rule rather than a preference:
 *
 * 1. **Nothing is written until the student confirms.** The header says it
 *    (`honesty.nothingAddedYet`), the confirm button is the only write, and the
 *    server's own `nothing_is_written_until_the_student_confirms` asserts the same
 *    thing about the rows.
 * 2. **Every item shows its evidence quote** — the exact source line it was read
 *    from. That quote is what lets the student check the item, which is the whole
 *    point of the step.
 * 3. **No confidence number appears anywhere** (A261). The score orders the list
 *    and flags an item, and both arrive already done: the list is pre-ordered
 *    lowest-confidence-first and each item carries a `needs_check` flag. This
 *    screen does not sort, and it does not render a score.
 * 4. **"Needs a check" is a word plus an icon, never a colour.** Gate 5(d): a state
 *    carried by colour alone is invisible to a monochrome print and to a reader who
 *    cannot distinguish the two hues.
 *
 * The edits are inline and the confirm body carries only what the student changed
 * (`review.ts`), because the server reads an absent field as "keep what the
 * extraction said" rather than "clear it".
 */
export function SyllabusReviewScreen({ draftId }: { draftId: string }) {
  const { t } = useTranslation()
  const session = useSession()
  const timeZone =
    session.status === 'signed-in' ? session.user.timezone : DEFAULT_TIMEZONE

  const [draft, setDraft] = useState<DraftBody | null>(null)
  const [drafts, setDrafts] = useState<ReviewDraft[] | null>(null)
  const [error, setError] = useState<ApiError | null>(null)
  const [saving, setSaving] = useState(false)
  const [written, setWritten] = useState<{ events: number; tasks: number; goals: number } | null>(
    null,
  )

  useEffect(() => {
    let live = true
    let timer: ReturnType<typeof setTimeout> | undefined

    const poll = async () => {
      try {
        const body = await fetchDraft(draftId)
        if (!live) return
        setDraft(body)
        setError(null)
        // The items are rebuilt from the draft only once, so a poll that returns
        // the same extraction cannot overwrite the student's edits.
        setDrafts((current) => current ?? draftFrom(reviewRows(body.items)))
      } catch (thrown) {
        const apiError = asApiError(thrown)
        if (!live) return
        // `conflict` is the *expected* answer while the extraction runs: the route
        // reports "still reading" that way, so the screen keeps polling rather
        // than showing it as a failure.
        if (apiError === null) return
        if (apiError.code === 'conflict') {
          timer = setTimeout(() => void poll(), 1500)
          return
        }
        setError(apiError)
      }
    }

    void poll()
    return () => {
      live = false
      clearTimeout(timer)
    }
  }, [draftId])

  const items = useMemo(() => (draft === null ? [] : reviewRows(draft.items)), [draft])

  const confirm = async () => {
    if (drafts === null) return
    setError(null)
    setSaving(true)
    try {
      const counts = await confirmSyllabus(draftId, confirmBody(drafts, items).items)
      setWritten(counts)
    } catch (thrown) {
      setError(asApiError(thrown))
    } finally {
      setSaving(false)
    }
  }

  const setDraftAt = (index: number, patch: Partial<ReviewDraft>) => {
    setDrafts((current) =>
      current === null
        ? current
        : current.map((row) => (row.index === index ? { ...row, ...patch } : row)),
    )
  }

  if (written !== null) {
    return (
      <div className="mx-auto flex w-full max-w-3xl flex-col gap-8 px-5 md:px-[32px] py-8">
        <PageHeader title={t('syllabus.review.title')} />
        <p className="note note-success wrap-anywhere">
          {t('syllabus.review.written', {
            events: written.events,
            tasks: written.tasks,
            goals: written.goals,
          })}
        </p>
        <div className="flex flex-wrap gap-2">
          <Link to="/planner" className="btn btn-primary">
            {t('syllabus.review.toPlanner')}
          </Link>
          <Link to="/tasks" className="btn btn-quiet">
            {t('nav.tasks')}
          </Link>
        </div>
      </div>
    )
  }

  return (
    <div className="mx-auto flex w-full max-w-3xl flex-col gap-8 px-5 md:px-[32px] py-8">
      <PageHeader title={t('syllabus.review.title')} subtitle={t('syllabus.review.subtitle')} />

      {/* The header line the honesty rule asks for: nothing exists yet. */}
      <p className="note wrap-anywhere">{t('honesty.nothingAddedYet')}</p>

      {error !== null ? (
        <ApiErrorNotice error={error} noun={t('syllabus.noun')} action={t('actions.retry')} />
      ) : null}

      {draft !== null && draft.status === 'failed' ? (
        <p className="note note-danger wrap-anywhere">
          {t('syllabus.review.failed', { message: draft.error ?? '' })}
        </p>
      ) : null}

      {draft === null ? <LoadingState label={t('syllabus.review.loading')} /> : null}

      {draft !== null && draft.status !== 'failed' && drafts !== null ? (
        <>
          {draft.course !== null && draft.course.name != null ? (
            <p className="text-small text-muted wrap-anywhere">
              {t('syllabus.review.course', {
                name: draft.course.name,
                code: draft.course.code ?? '',
              })}
            </p>
          ) : null}

          <Section
            title={t('syllabus.review.items')}
            count={drafts.length > 0 ? drafts.length : undefined}
          >
            <p className="text-small text-muted">
              {t('syllabus.review.needsCheck', { count: checkCount(items) })}
            </p>
            {drafts.length === 0 ? (
              <EmptyState
                title={t('syllabus.review.empty.title')}
                reason={t('syllabus.review.empty.reason')}
                padded={false}
              />
            ) : (
              <ul className="flex flex-col gap-3">
                {drafts.map((row) => {
                  const item = items.find((candidate) => candidate.index === row.index)
                  if (item === undefined) return null
                  return (
                    <li key={row.index} className="card flex flex-col gap-3 p-4">
                      <div className="flex flex-wrap items-center gap-2">
                        <span className="chip shrink-0">{t(`syllabus.kind.${item.kind}`)}</span>
                        {/* The flag is a word plus an icon, never a tone on its own
                            (gate 5(d)), and it is a real state rather than the
                            score: `needs_check` is the server's own boolean. */}
                        {item.needs_check ? (
                          <span className="chip chip-due shrink-0">
                            <Icon name="alert" size={16} />
                            {t('syllabus.review.needsCheckMark')}
                          </span>
                        ) : null}
                        {item.weight_percent !== null ? (
                          <span className="meta shrink-0">
                            {t('syllabus.review.weight', { percent: item.weight_percent })}
                          </span>
                        ) : null}
                      </div>

                      {/* **The input goes *inside* the label, and that is the app's
                          convention rather than a style choice** — `SourcePicker` and
                          `WorkspaceScreen`'s upload-kind radios are built the same way.
                          Two reasons, and the gate is the second: the `<label>` is the hit
                          area a finger reaches (a native checkbox box is ~13px, so
                          measuring the input alone reports a false failure on every
                          checkbox in the app), and the harness's `hitArea` resolves a
                          checkbox's target as `el.closest('label')` **or**
                          `label[for=…]`. As *siblings* with `htmlFor` it found the label,
                          but the wrapper carried only `min-h-11` while the input stayed
                          **61.3x22**, which is what the walk reported.

                          `has-[:focus-visible]` adds the row's ring **on top of** the
                          global `:focus-visible` rule's ring on the box — the same pair
                          `SourcePicker` carries. The box's ring alone is correct but
                          small; the row's is what makes the 44px target look like the
                          thing that has focus. */}
                      <label
                        htmlFor={`keep-${String(row.index)}`}
                        className="flex min-h-11 cursor-pointer items-center gap-2 text-body has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-ring"
                      >
                        <input
                          id={`keep-${String(row.index)}`}
                          type="checkbox"
                          checked={row.keep}
                          onChange={(event) => {
                            setDraftAt(row.index, { keep: event.target.checked })
                          }}
                        />
                        {t('syllabus.review.keep')}
                      </label>

                      <div className="flex flex-col gap-1">
                        <label className="label" htmlFor={`title-${String(row.index)}`}>
                          {t('syllabus.review.titleLabel')}
                        </label>
                        <input
                          id={`title-${String(row.index)}`}
                          className="field"
                          type="text"
                          value={row.title}
                          onChange={(event) => {
                            setDraftAt(row.index, { title: event.target.value })
                          }}
                        />
                      </div>

                      {item.when !== null ? (
                        <div className="flex flex-col gap-1">
                          <label className="label" htmlFor={`when-${String(row.index)}`}>
                            {t('syllabus.review.whenLabel')}
                          </label>
                          <input
                            id={`when-${String(row.index)}`}
                            className="field"
                            type="datetime-local"
                            value={localInput(row.when ?? item.when, timeZone)}
                            onChange={(event) => {
                              const next = new Date(event.target.value)
                              if (!Number.isNaN(next.getTime())) {
                                setDraftAt(row.index, { when: next.toISOString() })
                              }
                            }}
                          />
                          <p className="meta wrap-anywhere">
                            {t('syllabus.review.whenZone', {
                              when: formatInZone(new Date(row.when ?? item.when), timeZone),
                              zone: timeZone,
                            })}
                          </p>
                        </div>
                      ) : null}

                      {item.start_time !== null && item.end_time !== null ? (
                        <p className="meta">
                          {t('syllabus.review.meets', {
                            start: item.start_time,
                            end: item.end_time,
                            rule: row.rrule ?? '',
                          })}
                        </p>
                      ) : null}

                      {/* The evidence quote, which is the substance of this
                          screen: the exact source line the item was read from, so
                          the student can check it rather than trust it. */}
                      <blockquote className="note note-source wrap-anywhere">
                        {item.evidence}
                      </blockquote>

                      {row.topics.length > 0 ? (
                        <div className="flex flex-col gap-1">
                          <p className="meta">{t('syllabus.review.topics')}</p>
                          <div className="flex flex-wrap gap-2">
                            {row.topics.map((topic) => (
                              <span key={topic} className="chip wrap-anywhere">
                                {topic}
                              </span>
                            ))}
                          </div>
                        </div>
                      ) : null}
                    </li>
                  )
                })}
              </ul>
            )}
          </Section>

          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-primary"
              disabled={saving || keptCount(drafts) === 0}
              onClick={() => void confirm()}
            >
              {t('syllabus.review.confirm', { count: keptCount(drafts) })}
            </button>
          </div>
          {saving ? <LoadingState label={t('syllabus.review.saving')} /> : null}
        </>
      ) : null}
    </div>
  )
}

/**
 * The value a `datetime-local` input shows for an instant, in the student's zone.
 *
 * Built from `formatToParts` rather than from a fixed offset: the zone is the
 * student's IANA zone (default `Asia/Jakarta`), and a `+07:00` written here would
 * be a second, wrong answer for a device in another zone. The input reads local
 * wall-clock text, so the conversion is zone → parts → `YYYY-MM-DDTHH:MM`.
 */
function localInput(instant: string, timeZone: string): string {
  const parts = new Intl.DateTimeFormat('en-GB', {
    timeZone,
    hourCycle: 'h23',
    year: 'numeric',
    month: '2-digit',
    day: '2-digit',
    hour: '2-digit',
    minute: '2-digit',
  }).formatToParts(new Date(instant))

  const at = (type: string) => parts.find((part) => part.type === type)?.value ?? '00'
  return `${at('year')}-${at('month')}-${at('day')}T${at('hour')}:${at('minute')}`
}
