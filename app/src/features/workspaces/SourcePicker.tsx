import { useId } from 'react'

/**
 * Which sources an action may draw on.
 *
 * "None selected" means the whole workspace rather than no material in the chat
 * screen, and its hint says so: an empty selection that silently answered nothing
 * would look exactly like a workspace with nothing in it. The Studio panel's
 * selection is not the same thing — there, nothing selected means the form cannot
 * be submitted — so the hint follows the `legend` rather than assuming chat's
 * meaning.
 *
 * Each row is a 44px label wrapping its own checkbox, so the whole row is the
 * target at phone width and the label is what the screen reader reads.
 *
 * Extracted from `ChatScreen` so the Studio panel can label its own selection with
 * `legend` and explain an empty source list with `emptyHint`, without either screen
 * growing a second copy that drifts.
 */

/** The chat screen's wording, and the marker for "this is the chat meaning". */
const CHAT_LEGEND = 'Answer from'

export function SourcePicker({
  workspaceId,
  sources,
  selected,
  onChange,
  disabled,
  legend = CHAT_LEGEND,
  emptyHint = 'No indexed sources yet. Upload one in the Sources panel and this box will use it.',
}: {
  workspaceId: string
  sources: { id: string; title: string }[]
  selected: string[]
  onChange: (next: string[]) => void
  disabled: boolean
  /** What the selection is for. Defaults to the chat screen's wording. */
  legend?: string
  /** Shown instead of the rows when there is nothing to select. */
  emptyHint?: string
}) {
  const id = useId()

  if (sources.length === 0) {
    return <p className="text-xs text-ink-muted">{emptyHint}</p>
  }

  return (
    <fieldset aria-describedby={id} className="flex flex-col gap-1" disabled={disabled}>
      <legend className="label">{legend}</legend>
      <div className="flex max-h-32 flex-col gap-1 overflow-y-auto">
        {sources.map((source) => (
          <label
            key={source.id}
            className="flex min-h-11 items-center gap-2 rounded-field border border-rule px-3 py-2 text-sm has-[:focus-visible]:border-accent has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-accent"
          >
            <input
              type="checkbox"
              name={`source-${workspaceId}`}
              checked={selected.includes(source.id)}
              onChange={(event) => {
                onChange(
                  event.target.checked
                    ? [...selected, source.id]
                    : selected.filter((value) => value !== source.id),
                )
              }}
            />
            <span className="min-w-0 flex-1 break-words">{source.title}</span>
          </label>
        ))}
      </div>
      <span id={id} className="text-xs text-ink-muted">
        {selected.length === 0
          ? legend === CHAT_LEGEND
            ? 'Nothing selected, so the answer uses every ready source in this workspace.'
            : 'Nothing selected yet.'
          : `${String(selected.length)} of ${String(sources.length)} sources selected.`}
      </span>
    </fieldset>
  )
}
