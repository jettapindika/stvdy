import { describe, expect, it, vi } from 'vitest'

import { renderWithProviders } from '@/test/render'
import type { Proposal, ProposedBlock, StudySettings } from './plan'
import { ProposalPreview } from './ProposalPreview'

/**
 * The proposal grid's current-time line.
 *
 * **Why this file exists, when the browser gate already walks the calendar.** It does
 * walk it — `ProposalPreview` is rendered by `CalendarScreen` and by nothing else, so
 * the `calendar` surface is the only one that measures this component. But gate 5
 * reports **clipped text**, and this line's defect does not clip text: the label here
 * is `shrink-0`, so when the rule carried an unshrinkable `margin-left: <now %>` the
 * squeeze landed on the *rule* and collapsed it to nothing, leaving a bare "Now" with
 * no line at all. A zero-width hairline is not text, so the instrument that caught the
 * same construction in `TimeGrid` — where the label was the shrinkable item and
 * wrapped to three lines — is structurally blind to it here.
 *
 * So the property is pinned where it can be: no positional offset, and the rule is the
 * item that gives way. `TimeGrid.test.tsx` asserts the same three things on the
 * calendar's twin of this line, which is the point — one defect, two copies, and now
 * two guards.
 */

const ZONE = 'Asia/Jakarta'

/** 10:00 in Jakarta on Wednesday 10 June 2026 — inside the 07:00–22:00 window. */
const NOW = new Date('2026-06-10T03:00:00Z')

/** 05:00 the same Wednesday — the same row, but before the grid's window opens. */
const BEFORE_WINDOW = new Date('2026-06-09T22:00:00Z')

const SETTINGS: StudySettings = { start_hour: 7, end_hour: 22, max_study_min_per_day: 180 }

/**
 * One block on the Wednesday, and it is **load-bearing rather than decoration**: the
 * now-line lives inside the row's `<ul>`, which a day renders only when it has a block
 * or a busy interval (`ProposalPreview`'s `day.blocks.length === 0 && busyOn(...)`
 * branch). With an empty plan every row renders its "nothing scheduled" paragraph and
 * there is no line to assert — which is how the first draft of this file failed, and
 * also how a *second* test could pass for the wrong reason.
 *
 * Both cases below share this block, so the only thing that differs between them is the
 * time of day in `now` — which is what isolates `nowInWindow`.
 */
const WEDNESDAY_BLOCK: ProposedBlock = {
  starts_at: '2026-06-10T02:00:00Z', // 09:00 Jakarta
  ends_at: '2026-06-10T03:00:00Z', // 10:00 Jakarta
  goal_id: 'g1',
  topic: 'Struktur Data',
  kind: 'NewMaterial',
  pinned: false,
}

const PROPOSAL: Proposal = {
  blocks: [WEDNESDAY_BLOCK],
  day_load: [],
  deficit_min: 0,
  warnings: [],
}

function render(now: Date = NOW) {
  return renderWithProviders(
    <ProposalPreview
      proposal={PROPOSAL}
      now={now}
      timeZone={ZONE}
      settings={SETTINGS}
      busy={[]}
      goalTitles={() => 'Kriptografi Kunci Publik'}
      blocks={[WEDNESDAY_BLOCK]}
      onBlocks={vi.fn()}
      kept={[true]}
      onKept={vi.fn()}
    />,
  )
}

describe('the proposal grid’s current-time line', () => {
  it('is on today’s row only, and carries no positional offset', () => {
    const { container } = render()

    // Exactly one row is today, so exactly one line. This is also what proves the grid
    // rendered its week at all: a component that failed to build days would report zero
    // and pass the rest of this test vacuously.
    const lines = container.querySelectorAll('span.bg-border-strong')
    expect(lines).toHaveLength(1)

    const line = lines[0]
    if (line === undefined) throw new Error('the current-time line must render')

    // The defect, asserted as its absence. It was `marginLeft: <now as a % of the
    // window>`, which is not a flexible length: late in the day it claimed most of the
    // row, and because the label is `shrink-0` the rule was what collapsed. The design's
    // own current-time row (`411:26931`) is `label — flex-1 rule — label` with no offset.
    expect(line.style.marginLeft).toBe('')
    // And the rule is the item that yields, so the label keeps its intrinsic width.
    expect(line.className).toContain('flex-1')
    expect(line.className).toContain('min-w-0')
  })

  it('is absent when now is outside the grid’s window', () => {
    // *Whether* the line renders is a function of the `now` prop — through
    // `nowInWindow` — and never of a clock read at render. That is the property a clock
    // read would break, and the reason the component takes `now` as a prop at all.
    const { container } = render(BEFORE_WINDOW)
    expect(container.querySelectorAll('span.bg-border-strong')).toHaveLength(0)
  })
})
