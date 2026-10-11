import { CitationList } from '@/components/ui/Citation'
import { EmptyState, ErrorState, LoadingState, SkeletonRows } from '@/components/ui/States'
import { PageHeader, Section } from '@/components/layout/Section'

/**
 * A reference page for the design system, at `/styleguide`.
 *
 * It exists so the tokens and components can be reviewed in one place while the
 * product screens are still empty, and it is labelled as a reference so nobody
 * mistakes it for a product surface.
 *
 * Everything below uses obviously generic placeholders ("Source title", "page
 * 12") rather than plausible-looking sample data. A style guide full of invented
 * course names and student counts is exactly the fabrication R-23 and R-38
 * forbid, and it would be the easiest place to let it in unnoticed.
 */

const SWATCHES: { token: string; className: string; role: string }[] = [
  { token: 'paper', className: 'bg-paper', role: 'page ground' },
  { token: 'card', className: 'bg-card', role: 'panels that float' },
  { token: 'surface-sunk', className: 'bg-surface-sunk', role: 'fields, insets' },
  { token: 'rule', className: 'bg-rule', role: 'hairlines' },
  { token: 'rule-strong', className: 'bg-rule-strong', role: 'control borders' },
  { token: 'accent', className: 'bg-accent', role: 'identity: wordmark, links, illustration' },
  { token: 'accent-text', className: 'bg-accent-text', role: 'the same hue, for body text' },
  { token: 'success', className: 'bg-success', role: 'the checkmark teal' },
  { token: 'ink', className: 'bg-ink', role: 'body text and the action fill' },
  { token: 'ink-muted', className: 'bg-ink-muted', role: 'metadata' },
  { token: 'danger', className: 'bg-danger', role: 'errors' },
]

export function StyleguideScreen() {
  return (
    <div className="mx-auto flex max-w-3xl flex-col gap-10 px-5 py-8">
      <PageHeader
        title="Design reference"
        subtitle="Not a product screen. The tokens and components in one place, with placeholder content rather than sample data."
      />

      <Section title="Palette">
        <p className="max-w-prose text-sm text-ink-muted">
          Two cores plus one accent. The accent is rationed to the provenance
          layer and the single primary action per screen.
        </p>
        <ul className="flex flex-col divide-y divide-rule border-y border-rule">
          {SWATCHES.map((swatch) => (
            <li key={swatch.token} className="flex items-center gap-3 py-2">
              <span
                className={`h-6 w-6 shrink-0 rounded-sm border border-rule ${swatch.className}`}
                aria-hidden="true"
              />
              <span className="font-mono text-xs">{swatch.token}</span>
              <span className="ml-auto text-right text-xs text-ink-muted">{swatch.role}</span>
            </li>
          ))}
        </ul>
      </Section>

      <Section title="Type">
        <div className="flex flex-col gap-3">
          <p className="text-2xl font-semibold">Heading one</p>
          <p className="text-lg font-semibold">Heading two</p>
          <p className="font-semibold">Heading three</p>
          <p>
            Body text at 15px. IBM Plex Sans, chosen for its coverage of
            Latin-Extended characters, which matters for Indonesian, and because
            its narrower forms hold up in the dense reading views this product is
            mostly made of.
          </p>
          <p className="meta">Monospace metadata: page 12, 24 min, 2026-10-11</p>
          <p>
            A <a className="link" href="#palette">link in body copy</a> uses the darkened
            azure, because the raw brand blue is only AA at 18px and above.
          </p>
        </div>
      </Section>

      <Section title="Citation marks">
        <p className="max-w-prose text-sm text-ink-muted">
          The identity motif. A mark appears only next to something genuinely
          traceable, so it never implies grounding that is not there. A mark with
          a destination is a control; without one it is plain text.
        </p>
        <div className="flex flex-col gap-3 border-y border-rule py-3">
          <p className="text-sm">
            Encapsulation adds a header at each layer <CitationList citations={[{ type: 'chunk', label: 1, sourceTitle: 'Source title', page: 12 }]} />
            , and the overhead is predictable <CitationList citations={[
              { type: 'chunk', label: 2, sourceTitle: 'Source title', page: 4 },
              { type: 'chunk', label: 3, sourceTitle: 'Another source', page: null },
            ]} />
            . An outside result is marked separately <CitationList citations={[{ type: 'web', label: 'W1', url: 'https://example.com', title: 'Web result' }]} />.
          </p>
          <p className="text-sm">
            The interactive form is shown above as a web citation, which has a real
            destination. A chunk citation becomes a button only when a caller
            passes a handler; with none it renders as text rather than a control
            that swallows clicks.
          </p>
        </div>
      </Section>

      <Section title="Controls">
        <p className="max-w-prose text-sm text-ink-muted">
          Appearance only. These are inert samples, not controls: they carry
          <code> aria-disabled</code> and are outside the tab order, because a
          button that does nothing is a defect rather than a demo (R-26). A real
          screen wires each one to its own action.
        </p>
        <div className="flex flex-wrap items-center gap-2" aria-hidden="true">
          <button type="button" className="btn btn-primary" aria-disabled="true" tabIndex={-1}>
            Primary action
          </button>
          <button type="button" className="btn btn-secondary" aria-disabled="true" tabIndex={-1}>
            Secondary
          </button>
          <button type="button" className="btn btn-quiet" aria-disabled="true" tabIndex={-1}>
            Quiet
          </button>
          <button type="button" className="btn btn-primary" disabled>
            Disabled
          </button>
        </div>
        <label className="flex max-w-sm flex-col gap-1" aria-hidden="true">
          <span className="label">Field label</span>
          <input className="field" placeholder="Placeholder text" tabIndex={-1} readOnly />
        </label>
      </Section>

      <Section title="States">
        <div className="flex flex-col gap-3">
          <div className="panel">
            <EmptyState
              title="Empty state"
              reason="Names why it is empty and what fills it. The reason is a required prop, so a blank state cannot ship by forgetting."
            />
          </div>
          <ErrorState
            title="Error state"
            reason="Names what failed and what to do about it."
          />
          <div className="panel">
            <LoadingState label="Loading a named thing, not just loading" />
          </div>
          <div className="panel">
            <SkeletonRows label="Loading rows" />
          </div>
        </div>
      </Section>
    </div>
  )
}
