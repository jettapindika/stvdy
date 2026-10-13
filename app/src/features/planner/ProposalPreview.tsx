import { useState } from 'react'
import { useTranslation } from 'react-i18next'

import { formatInZone } from '@/lib/time'
import {
  GRID_END_HOUR,
  GRID_MINUTES,
  GRID_START_HOUR,
  blockSubject,
  gridDays,
  localMinutesOfDay,
  moveBlock,
  todayKey,
  warningCopy,
  weekDays,
  type Interval,
  type MoveRefusal,
  type Proposal,
  type ProposedBlock,
  type StudySettings,
} from './plan'

/**
 * The proposal, as a proposal.
 *
 * Three things this component is built to get right, and each is a gate rather
 * than a preference.
 *
 * **It is a "Suggested …" plan, and accepting is a separate act** (honesty gate
 * 7(e)). `POST /plans/accept` is what writes events, tasks and goals; nothing
 * before it does, and the label and the `honesty.nothingAddedYet` line say so.
 *
 * **The actions are not here; they are in the plan banner above.** The design puts
 * `Accept all` / `Accept selected` / `Dismiss` at the top of the suggested plan and
 * leaves the grid below as the thing being decided about, which is also what keeps
 * one screen from having two primary actions. So this component is **controlled**:
 * the banner owns the block list and the per-block keep flags, and everything here
 * asks the banner to change them. The two cannot disagree about what will be
 * written, because there is only one copy.
 *
 * **Selection is per block, and `Accept all` is not the same button as `Accept
 * selected`.** A plan that could only be taken or left whole would force a student
 * to drop a session they wanted in order to avoid one they did not, and the design
 * asks for both. A kept block that the student then unchecks is not written.
 *
 * **The grid is a table, one row per day.** A grid of absolutely-positioned
 * `<div>`s is a grid a keyboard cannot enter and a screen reader cannot read
 * (A244); a row per day is the same information, reflows to one line per block by
 * construction, and is what survives 200% text on a phone. Each block's position
 * in the 07:00–22:00 day is drawn as a proportional bar *inside* the row, so the
 * position is visible without a pixel that has to be measured.
 *
 * **The drag is a keyboard control, because a pointer drag is unreachable.** Gate
 * 5 asks for the arrow keys to move a block, so a block is a `<button>` and the
 * arrow keys move it by 15 minutes; a drag-and-drop handler alone would be a
 * control no keyboard user could operate and no jsdom test could drive. A move
 * that would overlap a class, leave the study window or exceed the day's maximum
 * is refused **here** with a sentence, because the server's re-validation (A264)
 * refuses the whole batch and a student should not learn that after a round trip.
 *
 * **A pinned block is the student's own placement.** Moving one marks it pinned,
 * and the grid says what that means: a replan keeps it exactly (contract § 6 step
 * 7). The label is a word plus a note, never a colour.
 */

/** The minutes one arrow-key press moves a block. */
const NUDGE_MIN = 15

/** `08:00` for an instant, in the student's zone, through the shared helper. */
function clockLabel(instant: string, timeZone: string): string {
  return formatInZone(new Date(instant), timeZone, { hour: '2-digit', minute: '2-digit' })
}

/** The busy rows that overlap a local day, as read-only rows in its block list. */
function busyOn(day: string, busy: Interval[], timeZone: string): Interval[] {
  return busy.filter((interval) => todayKey(new Date(interval.starts_at), timeZone) === day)
}

export function ProposalPreview({
  proposal,
  now,
  timeZone,
  settings,
  busy,
  goalTitles,
  blocks,
  onBlocks,
  kept,
  onKept,
}: {
  proposal: Proposal
  /** The instant the grid was opened, fixed for the life of the mount. */
  now: Date
  timeZone: string
  settings: StudySettings
  /** The busy intervals the grid was given: classes, exams, anything not this plan. */
  busy: Interval[]
  goalTitles: (goalId: string) => string | undefined
  /** The block list, owned by the banner: a move changes it for the banner too. */
  blocks: ProposedBlock[]
  onBlocks: (next: ProposedBlock[]) => void
  /** One flag per block, in the same order as `blocks`. */
  kept: boolean[]
  onKept: (next: boolean[]) => void
}) {
  const { t } = useTranslation()
  const [refusal, setRefusal] = useState<MoveRefusal | null>(null)

  const today = todayKey(now, timeZone)
  const days = weekDays(now, timeZone)
  const grid = gridDays(blocks, days, timeZone)
  const nowMin = localMinutesOfDay(now, timeZone)
  const nowInWindow = nowMin >= GRID_START_HOUR * 60 && nowMin <= GRID_END_HOUR * 60

  const nudge = (index: number, deltaMinutes: number) => {
    const result = moveBlock(blocks, index, deltaMinutes, {
      timeZone,
      settings,
      busy,
    })
    if (result.ok) {
      onBlocks(result.blocks)
      setRefusal(null)
      return
    }
    setRefusal(result.reason)
  }

  return (
    <div className="flex flex-col gap-4">
      <div className="raised flex flex-col gap-2 p-3">
        <p className="label">{t('honesty.suggested', { what: t('planner.proposal.what') })}</p>
        <p className="text-small text-muted">{t('honesty.nothingAddedYet')}</p>
        {/* The deficit is a real computed number, so it is shown as one. A plan
            that could not fit everything says how much it could not fit rather
            than looking complete. */}
        {proposal.deficit_min > 0 ? (
          <p className="note note-warning wrap-anywhere">
            {t('planner.deficit', { minutes: proposal.deficit_min })}
          </p>
        ) : null}
        {proposal.warnings.map((warning) => {
          const copy = warningCopy(warning, goalTitles)
          return (
            <p key={copy.key} className="note note-warning wrap-anywhere">
              {copy.key === 'planner.overloaded'
                ? t('planner.overloaded', { count: copy.count })
                : t('planner.pastDeadline', { goal: copy.goal })}
            </p>
          )
        })}
        <p className="text-small text-muted wrap-anywhere">{t('planner.pinnedNote')}</p>
      </div>

      {refusal !== null ? (
        <p className="note note-warning wrap-anywhere">{t(`planner.refused.${refusal}`)}</p>
      ) : null}

      <div className="card overflow-x-auto p-4">
        <table className="w-full min-w-0 border-collapse text-left">
          <caption className="meta pb-2 text-left">
            {t('planner.grid.caption', {
              start: `${String(GRID_START_HOUR).padStart(2, '0')}:00`,
              end: `${String(GRID_END_HOUR).padStart(2, '0')}:00`,
            })}
          </caption>
          <thead>
            <tr>
              <th scope="col" className="label border-b border-border pb-2">
                {t('planner.grid.day')}
              </th>
              <th scope="col" className="label border-b border-border pb-2">
                {t('planner.grid.blocks')}
              </th>
            </tr>
          </thead>
          <tbody>
            {grid.map((day) => (
              <tr key={day.date} className="border-b border-border align-top">
                <th scope="row" className="min-w-0 py-3 pr-3 text-left font-normal">
                  <span className="flex flex-col gap-1">
                    <span className="text-body font-medium text-text">
                      {formatInZone(new Date(`${day.date}T12:00:00Z`), timeZone, {
                        weekday: 'long',
                      })}
                    </span>
                    <span className="meta">
                      {formatInZone(new Date(`${day.date}T12:00:00Z`), timeZone, {
                        day: 'numeric',
                        month: 'short',
                      })}
                    </span>
                    {day.date === today ? (
                      <span className="chip self-start">{t('planner.grid.today')}</span>
                    ) : null}
                  </span>
                </th>
                <td className="min-w-0 py-3">
                  {day.blocks.length === 0 && busyOn(day.date, busy, timeZone).length === 0 ? (
                    <p className="text-small text-subtle">{t('planner.grid.empty')}</p>
                  ) : (
                    <ul className="flex flex-col gap-3">
                      {busyOn(day.date, busy, timeZone).map((interval) => (
                        <li key={`busy-${interval.starts_at}`} className="flex min-w-0 flex-wrap gap-2">
                          <span className="meta shrink-0">
                            {clockLabel(interval.starts_at, timeZone)}
                          </span>
                          <span className="min-w-0 break-words text-small text-muted wrap-anywhere">
                            {t('planner.grid.busy')}
                          </span>
                        </li>
                      ))}
                      {day.blocks.map((placed, position) => {
                        const index = blocks.indexOf(placed.block)
                        const subject = blockSubject(placed.block, goalTitles)
                        const left = ((placed.start_min - GRID_START_HOUR * 60) / GRID_MINUTES) * 100
                        const width = ((placed.end_min - placed.start_min) / GRID_MINUTES) * 100
                        const keptBlock = subject.kind === 'kept'
                        return (
                          <li key={`${placed.block.starts_at}-${String(position)}`} className="flex min-w-0 flex-col gap-1">
                            <span className="flex min-w-0 items-start gap-2">
                              {/* The keep flag, beside the block rather than
                                  inside it: a `<button>` may not contain another
                                  control, so nesting this input would be invalid
                                  markup that a browser silently reparents. Its
                                  accessible name is the block's own subject and
                                  time, so `Accept selected` has something the
                                  student can name. A kept block is checked by
                                  default — the proposal arrives with everything
                                  in it, which is what `Accept all` means. */}
                              <input
                                id={`keep-${placed.block.starts_at}-${String(position)}`}
                                className="mt-3 shrink-0"
                                type="checkbox"
                                checked={kept[index] ?? true}
                                aria-label={t('planner.proposal.selectBlock', {
                                  subject: keptBlock ? t('planner.pinned') : subject.text,
                                  start: clockLabel(placed.block.starts_at, timeZone),
                                  end: clockLabel(placed.block.ends_at, timeZone),
                                })}
                                onChange={(event) =>
                                  onKept(
                                    blocks.map((_block, at) =>
                                      at === index ? event.target.checked : (kept[at] ?? true),
                                    ),
                                  )
                                }
                              />
                            <button
                              type="button"
                              // The label carries what the block is and what the
                              // arrow keys do, because the position on the grid is
                              // a visual fact and the keyboard user has no grid.
                              aria-label={
                                keptBlock
                                  ? t('planner.grid.keptLabel', {
                                      start: clockLabel(placed.block.starts_at, timeZone),
                                      end: clockLabel(placed.block.ends_at, timeZone),
                                    })
                                  : t('planner.grid.moveLabel', {
                                      subject: subject.text,
                                      start: clockLabel(placed.block.starts_at, timeZone),
                                      end: clockLabel(placed.block.ends_at, timeZone),
                                    })
                              }
                              // A kept block is not movable, and the keys are not bound rather
                              // than bound-and-refused: the server resolves it by its exact
                              // range, so a move makes the whole accept a 409 (A265).
                              onKeyDown={
                                keptBlock
                                  ? undefined
                                  : (event) => {
                                      if (event.key === 'ArrowLeft') {
                                        event.preventDefault()
                                        nudge(index, -NUDGE_MIN)
                                      }
                                      if (event.key === 'ArrowRight') {
                                        event.preventDefault()
                                        nudge(index, NUDGE_MIN)
                                      }
                                      if (event.key === 'ArrowUp') {
                                        event.preventDefault()
                                        nudge(index, -NUDGE_MIN * 4)
                                      }
                                      if (event.key === 'ArrowDown') {
                                        event.preventDefault()
                                        nudge(index, NUDGE_MIN * 4)
                                      }
                                    }
                              }
                              className="flex min-h-11 w-full min-w-0 flex-wrap items-center gap-x-2 gap-y-1 rounded-field px-2 py-2 text-left hover:bg-surface-raised focus:outline focus:outline-2 focus:outline-offset-2 focus:outline-ring"
                            >
                              <span className="meta shrink-0">
                                {clockLabel(placed.block.starts_at, timeZone)}–
                                {clockLabel(placed.block.ends_at, timeZone)}
                              </span>
                              <span className="min-w-0 break-words text-body text-text wrap-anywhere">
                                {keptBlock ? t('planner.pinned') : subject.text}
                              </span>
                              {keptBlock ? (
                                <span className="chip shrink-0">{t('planner.pinnedMark')}</span>
                              ) : null}
                              {placed.block.kind.toLowerCase() === 'review' ? (
                                <span className="chip shrink-0">{t('planner.review')}</span>
                              ) : null}
                              {placed.block.pinned && !keptBlock ? (
                                <span className="chip shrink-0">{t('planner.moved')}</span>
                              ) : null}
                            </button>
                            </span>
                            <span
                              aria-hidden="true"
                              className="h-1 rounded-pill bg-primary"
                              style={{
                                marginLeft: `${String(Math.max(0, left))}%`,
                                width: `${String(Math.max(2, width))}%`,
                              }}
                            />
                          </li>
                        )
                      })}
                      {/* The current-time line, on today's row only, positioned
                          from the instant the grid was opened — never from a
                          fresh `new Date()`, so two renders of the same grid
                          cannot disagree about where the line is. */}
                      {day.date === today && nowInWindow ? (
                        <li className="flex min-w-0 items-center gap-2">
                          <span className="meta shrink-0">{t('planner.grid.now')}</span>
                          <span
                            aria-hidden="true"
                            className="h-px flex-1 bg-border-strong"
                            style={{
                              marginLeft: `${
                                String(
                                  ((nowMin - GRID_START_HOUR * 60) / GRID_MINUTES) * 100,
                                )
                              }%`,
                            }}
                          />
                        </li>
                      ) : null}
                    </ul>
                  )}
                </td>
              </tr>
            ))}
          </tbody>
        </table>
      </div>

    </div>
  )
}
