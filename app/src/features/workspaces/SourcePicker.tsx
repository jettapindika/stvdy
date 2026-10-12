import { useId } from 'react'
import { useTranslation } from 'react-i18next'

/**
 * Which sources an action may draw on.
 *
 * "None selected" means the whole workspace rather than no material in the chat
 * screen, and its hint says so: an empty selection that silently answered nothing
 * would look exactly like a workspace with nothing in it. The Studio panel's
 * selection is not the same thing — there, nothing selected means the form cannot
 * be submitted — so the hint follows whether the caller supplied its own `legend`
 * rather than assuming chat's meaning.
 *
 * Each row is a 44px label wrapping its own checkbox, so the whole row is the
 * target at phone width and the label is what the screen reader reads.
 *
 * Extracted from `ChatScreen` so the Studio panel can label its own selection with
 * `legend` and explain an empty source list with `emptyHint`, without either screen
 * growing a second copy that drifts.
 */
export function SourcePicker({
  workspaceId,
  sources,
  selected,
  onChange,
  disabled,
  legend,
  emptyHint,
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
  const { t } = useTranslation()
  const id = useId()

  // The chat screen's wording, resolved here because a default cannot be a
  // module-scope string: there is no language at module scope.
  const chatLegend = t('workspaces.sourcePicker.legend')
  const label = legend ?? chatLegend

  if (sources.length === 0) {
    return (
      <p className="text-xs text-muted">{emptyHint ?? t('workspaces.sourcePicker.emptyHint')}</p>
    )
  }

  return (
    <fieldset aria-describedby={id} className="flex flex-col gap-1" disabled={disabled}>
      <legend className="label">{label}</legend>
      <div className="flex max-h-32 flex-col gap-1 overflow-y-auto">
        {sources.map((source) => (
          /* `shrink-0` is load-bearing. The list is capped at `max-h-32` (128px,
          which is 256px at 200% text), so when the rows' natural heights exceed
          the cap the flex container shrinks them — and a row is a label whose text
          cannot shrink with it, so the text overflowed the box it was clipped to:
          measured at 200% text on a 390px phone, a row's `scrollHeight` was 147
          against a `clientHeight` of 135, i.e. a line of the source's title cut off
          inside a scrolling list that was scrolling. Refusing to shrink makes the
          container scroll instead, which is what `overflow-y-auto` is there for.
          `max-h-32` stays: at 100% the two rows fit in it and nothing scrolls. */
          <label
            key={source.id}
            className="flex min-h-11 shrink-0 items-center gap-2 rounded-field border border-border px-3 py-2 text-sm has-[:focus-visible]:border-primary has-[:focus-visible]:outline has-[:focus-visible]:outline-2 has-[:focus-visible]:outline-offset-2 has-[:focus-visible]:outline-ring"
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
      <span id={id} className="text-xs text-muted">
        {selected.length === 0
          ? legend === undefined
            ? t('workspaces.sourcePicker.nothingSelectedAllReady')
            : t('workspaces.sourcePicker.nothingSelected')
          : t('workspaces.sourcePicker.selectedCount', {
              selected: selected.length,
              total: sources.length,
            })}
      </span>
    </fieldset>
  )
}
