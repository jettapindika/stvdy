# Content CRUD Management System

**Date**: 2026-10-15
**Approach**: Per-entity inline management (Approach A)

## Problem

Deck deletion exists end-to-end, but quizzes, reports, and cheatsheets cannot be deleted, renamed, reordered, or bulk-deleted. Workspace deletion orphans content (decks/quizzes/reports/cheatsheets stay live and count against monthly limits). Users need full CRUD management for all content types.

## Scope

| Action | Decks | Quizzes | Reports | Cheatsheets |
|---|---|---|---|---|
| Create | ✅ exists | ✅ exists | ✅ exists | ✅ exists |
| Read | ✅ exists | ✅ exists | ✅ exists | ✅ exists |
| Rename | **new** | **new** | ✅ exists (via outline PATCH) | **new** |
| Delete (individual) | ✅ exists | **new** | **new** | **new** |
| Delete (from detail screen) | ✅ exists (StudyScreen) | **new** (QuizScreen) | **new** (ReportScreen) | **new** (CheatsheetScreen) |
| Bulk delete | **new** | **new** | **new** | **new** |
| Reorder | **new** | **new** | **new** | **new** |
| Cascade on workspace delete | **new** | **new** | **new** | **new** |

## 1. Backend — Delete Routes

Three new delete endpoints mirroring `delete_deck` (`flashcards/routes.rs:520-563`):

### `DELETE /quizzes/{id}` → 204

1. `owned_quiz(state, user_id, quiz_id)` — 404 if missing/other-account/tombstoned
2. Begin transaction
3. Tombstone `quizzes` row: `SET deleted_at = now(), updated_at = now(), credits_spent = CASE WHEN status = 'generating' THEN 0 ELSE credits_spent END WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL`
4. Tombstone `quiz_questions`: `SET deleted_at = now(), updated_at = now() WHERE quiz_id = $1 AND deleted_at IS NULL`
5. `quiz_attempts` has no `deleted_at` column — left intact (append-only history, FK cascades on real DELETE during account purge)
6. Commit, return 204

### `DELETE /reports/{id}` → 204

1. `owned_report(state, user_id, report_id)` — 404
2. Begin transaction
3. Tombstone `reports` row (same credit-zeroing pattern)
4. `report_revisions` has no `deleted_at` — left intact (cascades on real DELETE during purge)
5. Commit, return 204

### `DELETE /cheatsheets/{id}` → 204

1. `owned_cheatsheet(state, user_id, cheatsheet_id)` — 404
2. Begin transaction
3. Tombstone `cheatsheets` row (same credit-zeroing pattern)
4. No child tables
5. Object-storage keys (`blocks_key`, `html_key`, `pdf_key`) are left intact — purge job handles them (same as today)
6. Commit, return 204

**Credit safety**: Zeroing `credits_spent` on `generating` rows prevents `fail_and_refund` from refunding a deliberately deleted item. `failed` rows' charges are left for the boot sweep. This matches `delete_deck` exactly.

**Generation jobs**: All four generators (`generate.rs`, `generate_quiz.rs`, `report.rs`, `cheatsheet.rs`) already bail when `deleted_at IS NOT NULL`, so no cancellation API is needed.

## 2. Backend — Rename Routes

Three new PATCH endpoints (reports already have `PATCH /reports/{id}`):

### `PATCH /decks/{id}` → updated deck JSON

Body: `{ "title": "string" }`
- Validate title: non-empty, max 200 chars
- `owned_deck` for ownership
- `UPDATE decks SET title = $3, updated_at = now() WHERE id = $1 AND user_id = $2 AND deleted_at IS NULL`
- Return updated `DeckJson` with card count

### `PATCH /quizzes/{id}` → updated quiz JSON

Body: `{ "title": "string" }`
- Same validation and pattern as decks
- Return updated `QuizJson` with question count

### `PATCH /cheatsheets/{id}` → updated cheatsheet JSON

Body: `{ "title": "string" }`
- Same validation and pattern
- Return updated `CheatsheetJson`

Reports: `PATCH /reports/{id}` already exists and accepts `title` as part of the outline update body. The frontend already has `useUpdateOutline`. A rename-only call sends the existing outline unchanged with the new title.

## 3. Backend — Workspace Delete Cascade

Extend `workspaces::delete_one` (`workspaces.rs:305-374`) to tombstone all content inside the transaction, after tombstoning sources:

```sql
-- Tombstone decks + their flashcards
UPDATE decks SET deleted_at = now(), updated_at = now(),
    credits_spent = CASE WHEN status = 'generating' THEN 0 ELSE credits_spent END
  WHERE workspace_id = $1 AND deleted_at IS NULL;
UPDATE flashcards SET deleted_at = now(), updated_at = now()
  WHERE deck_id IN (SELECT id FROM decks WHERE workspace_id = $1)
    AND deleted_at IS NULL;

-- Tombstone quizzes + their questions
UPDATE quizzes SET deleted_at = now(), updated_at = now(),
    credits_spent = CASE WHEN status = 'generating' THEN 0 ELSE credits_spent END
  WHERE workspace_id = $1 AND deleted_at IS NULL;
UPDATE quiz_questions SET deleted_at = now(), updated_at = now()
  WHERE quiz_id IN (SELECT id FROM quizzes WHERE workspace_id = $1)
    AND deleted_at IS NULL;

-- Tombstone reports
UPDATE reports SET deleted_at = now(), updated_at = now(),
    credits_spent = CASE WHEN status = 'generating' THEN 0 ELSE credits_spent END
  WHERE workspace_id = $1 AND deleted_at IS NULL;

-- Tombstone cheatsheets
UPDATE cheatsheets SET deleted_at = now(), updated_at = now(),
    credits_spent = CASE WHEN status = 'generating' THEN 0 ELSE credits_spent END
  WHERE workspace_id = $1 AND deleted_at IS NULL;
```

This fixes the gap where orphaned content continued counting against monthly entitlement limits (`entitlements.rs:891-940` filters on each table's own `deleted_at`).

## 4. Backend — Bulk Delete

### `POST /workspaces/{id}/bulk-delete` → 204

Body:
```json
{
  "deck_ids": ["uuid"],
  "quiz_ids": ["uuid"],
  "report_ids": ["uuid"],
  "cheatsheet_ids": ["uuid"]
}
```

All arrays optional (default empty). At least one id required (400 if all empty).

One transaction:
1. Verify workspace ownership via `owned_name`
2. For each non-empty array, tombstone the parent rows (verified `workspace_id = $workspace AND user_id = $user AND deleted_at IS NULL`)
3. Tombstone children (`flashcards` for decks, `quiz_questions` for quizzes)
4. Same credit-zeroing for generating rows
5. Commit, 204

Validation: any id not belonging to the workspace is silently skipped (idempotent — already deleted or wrong workspace). This matches `delete_deck`'s idempotency: a second DELETE on the same id returns 404 from `owned_*`, but bulk delete does not fail the whole batch over one stale id.

## 5. Backend — Reorder

### Migration

```sql
-- 0041_content_sort_order.sql
ALTER TABLE decks ADD COLUMN sort_order int NOT NULL DEFAULT 0;
ALTER TABLE quizzes ADD COLUMN sort_order int NOT NULL DEFAULT 0;
ALTER TABLE reports ADD COLUMN sort_order int NOT NULL DEFAULT 0;
ALTER TABLE cheatsheets ADD COLUMN sort_order int NOT NULL DEFAULT 0;
```

Default 0 means existing rows sort by `created_at DESC` (the current behavior, preserved by `ORDER BY sort_order, created_at DESC`).

### `PATCH /workspaces/{id}/content-order` → 204

Body:
```json
{
  "deck_ids": ["uuid"],
  "quiz_ids": ["uuid"],
  "report_ids": ["uuid"],
  "cheatsheet_ids": ["uuid"]
}
```

Sets `sort_order` from array index (0-based). One transaction. Ownership verified. Items not in the array are left untouched — their existing `sort_order` stays. When all rows start at 0, newly reordered items sort first (lower indices), and unreordered items group after them by `created_at DESC`. A full reorder sends every live item, so partial reorders are only for "move one item" cases.

### List query changes

All four list queries (`list_decks`, `list_quizzes`, `list_reports`, `list_cheatsheets`) change `ORDER BY` from `created_at DESC` to `sort_order ASC, created_at DESC`.

## 6. Frontend — Query Hooks

### New hooks

| Module | Hook | Method | Invalidates |
|---|---|---|---|
| `flashcards.ts` | `useRenameDeck()` | `PATCH /decks/:id` | `flashcardKeys.decks(wid)`, `['dashboard']` |
| `quizzes.ts` | `useDeleteQuiz(workspaceId)` | `DELETE /quizzes/:id` | `quizKeys.list(wid)`, `['dashboard']` |
| `quizzes.ts` | `useRenameQuiz()` | `PATCH /quizzes/:id` | `quizKeys.list(wid)`, `quizKeys.detail(id)` |
| `reports.ts` | `useDeleteReport(workspaceId)` | `DELETE /reports/:id` | `reportKeys.list(wid)`, `['dashboard']` |
| `cheatsheets.ts` | `useDeleteCheatsheet(workspaceId)` | `DELETE /cheatsheets/:id` | `cheatsheetKeys.list(wid)`, `['dashboard']` |
| `cheatsheets.ts` | `useRenameCheatsheet()` | `PATCH /cheatsheets/:id` | `cheatsheetKeys.list(wid)`, `cheatsheetKeys.detail(id)` |
| `workspaces.ts` | `useBulkDelete(workspaceId)` | `POST /workspaces/:id/bulk-delete` | All content keys for workspace, `['dashboard']` |
| `workspaces.ts` | `useReorderContent(workspaceId)` | `PATCH /workspaces/:id/content-order` | All content list keys for workspace |

All follow the existing mutation convention: `invalidateQueries` in `onSuccess`, no optimistic updates, no `removeQueries`.

## 7. Frontend — Row UI (List Views)

### State machine per row

Each content row (`DeckRow`, `QuizRow`, `ReportRow`, new `CheatsheetRow`) gains a mode state:

```typescript
const [mode, setMode] = useState<'idle' | 'renaming' | 'deleting'>('idle')
```

### idle mode

Existing layout plus two new buttons:
- Rename: `btn btn-quiet text-sm` → switches to `renaming`
- Delete: `btn btn-quiet text-danger text-sm` → switches to `deleting`

### deleting mode

In-row confirm block (`note note-danger`), matching `DeckRow`:
- Confirm text states cascade consequence and irreversibility
- Destructive button is `autoFocus`, shows "Deleting…" while pending
- Cancel calls `mutation.reset()` before closing
- Error via `ApiErrorNotice` with domain noun

Cascade info per type:
- Deck: "X cards will be permanently deleted" (exists)
- Quiz: "X questions and all attempt history will be lost"
- Report: "The outline and all generated sections will be lost"
- Cheatsheet: "The cheatsheet will be permanently deleted"

### renaming mode

Inline `<form>` with:
- `<input className="field" defaultValue={title}>` with autoFocus
- Save button: `btn btn-primary text-sm`, disabled while pending
- Cancel button: `btn btn-quiet text-sm`, calls `mutation.reset()`
- Submit calls the rename mutation
- On success: `setMode('idle')`

Pattern matches `DeviceRow` rename and `WorkspaceCard` rename.

### CheatsheetRow (new component)

Currently cheatsheets are listed inside `ReportPanel` via `CheatsheetForm`. Extract a `CheatsheetRow` component following the same structure as `QuizRow`/`ReportRow` with the mode state machine.

## 8. Frontend — Detail Screen Delete

Add delete to each detail screen's header/actions area:

| Screen | Location | On success |
|---|---|---|
| `QuizScreen` | Header area beside quiz title | `navigate(`/workspaces/${workspaceId}`)` |
| `ReportScreen` | Header area beside report title | `navigate(`/workspaces/${workspaceId}`)` |
| `CheatsheetScreen` | Actions area (beside re-render + export) | `navigate(`/workspaces/${workspaceId}`)` |
| `StudyScreen` | Already exists ✅ | Already navigates to `/study` |

Same inline confirm pattern. On QuizScreen, placed after the title and before the question list. On ReportScreen, in the header. On CheatsheetScreen, in `CheatsheetActions`.

Rename on detail screens: a rename button that switches the title to an inline `<input>` form (enter to save, escape to cancel). Same form pattern as the list view rows.

## 9. Frontend — Bulk Delete

### Selection mode

Each panel header gains a "Select" toggle:
- `btn btn-quiet text-sm` with checkbox icon
- When active, each row shows a checkbox before the title
- Selected items tracked in `Set<string>` state lifted to the panel

### Bulk delete toolbar

When `selection.size > 0`, a sticky toolbar appears at the bottom of the panel:
- Shows count: "3 selected"
- "Delete" button: `btn btn-danger text-sm`
- On click: inline confirm block replaces the toolbar with cascade warning + confirm/cancel
- Uses `useBulkDelete` hook

### Deselection

- Clicking a row's checkbox toggles it
- "Deselect all" link in toolbar
- Exiting select mode clears selection
- Successful delete clears deleted items from selection

## 10. Frontend — Reorder

Arrow buttons on each row (matching `OutlineEditor`'s reorder pattern):
- Up arrow: `btn btn-quiet p-1` with `Icon name="chevron-up"`
- Down arrow: `btn btn-quiet p-1` with `Icon name="chevron-down"`
- First item: up disabled. Last item: down disabled.
- On click: swap locally, then debounce a `useReorderContent` call (300ms)

No drag library — consistent with the codebase's button-based reorder in `OutlineEditor`.

## 11. i18n Keys

### English (`en.json`)

```
studio.quiz.deleteConfirm: "Delete \"{{name}}\"?"
studio.quiz.deleteQuestions_one: "{{count}} question and all attempt history will be permanently deleted."
studio.quiz.deleteQuestions_other: "{{count}} questions and all attempt history will be permanently deleted."
studio.quiz.deleteNoQuestions: "This quiz has no questions yet."
studio.quiz.deleting: "Deleting…"
studio.quiz.deleteIt: "Delete quiz"
studio.quiz.rename: "Rename"

studio.report.deleteConfirm: "Delete \"{{name}}\"?"
studio.report.deleteSections_one: "The outline and {{count}} generated section will be permanently deleted."
studio.report.deleteSections_other: "The outline and {{count}} generated sections will be permanently deleted."
studio.report.deleteNoSections: "This report has no generated sections yet."
studio.report.deleting: "Deleting…"
studio.report.deleteIt: "Delete report"

studio.cheatsheet.deleteConfirm: "Delete \"{{name}}\"?"
studio.cheatsheet.deleteBody: "The cheatsheet will be permanently deleted."
studio.cheatsheet.deleting: "Deleting…"
studio.cheatsheet.deleteIt: "Delete cheatsheet"
studio.cheatsheet.rename: "Rename"

studio.deck.rename: "Rename"

studio.bulk.select: "Select"
studio.bulk.selected_one: "{{count}} selected"
studio.bulk.selected_other: "{{count}} selected"
studio.bulk.deleteConfirm: "Delete {{count}} items?"
studio.bulk.deleteBody: "This action cannot be undone."
studio.bulk.deleting: "Deleting…"
studio.bulk.deleteIt: "Delete all"
studio.bulk.deselectAll: "Deselect all"

errors.nouns.quiz: "quiz"
errors.nouns.report: "report"
errors.nouns.cheatsheet: "cheatsheet"
```

### Indonesian (`id.json`)

Matching keys with Indonesian translations. Plurals use `_one`/`_other` forms.

## 12. Registration Obligations

### Route sweep (`crates/server/tests/route_sweep.rs`)

Add entries to the `ROUTES` table for:
- `DELETE /quizzes/{id}` — read-only (no plan gate)
- `DELETE /reports/{id}` — read-only
- `DELETE /cheatsheets/{id}` — read-only
- `PATCH /decks/{id}` — read-only
- `PATCH /quizzes/{id}` — read-only
- `PATCH /cheatsheets/{id}` — read-only
- `POST /workspaces/{id}/bulk-delete` — read-only
- `PATCH /workspaces/{id}/content-order` — read-only

### Tamper tests (`crates/server/tests/tamper.rs`)

Add `GET|DELETE|PATCH /quizzes/{id}`, `DELETE /reports/{id}`, `GET|DELETE|PATCH /cheatsheets/{id}` to the cross-tenant route list.

### API docs (`docs/API.md`)

Add endpoint documentation sections and update the classification tables.

## 13. Tests

### Backend tests

- `quizzes.rs`: delete test (tombstones questions, leaves attempts, zeros generating credits), rename test
- `reports.rs`: delete test (tombstones report, leaves revisions), title update via existing PATCH
- `cheatsheets.rs`: delete test (tombstones row, leaves storage keys), rename test
- `flashcards.rs`: rename test for decks
- `workspaces.rs`: cascade delete test (verify all content tombstoned, entitlement counts drop)
- Bulk delete test (mixed content types, idempotent on already-deleted)
- Reorder test (sort_order persisted, list order matches)

### Frontend

Existing a11y and phone layout test sweeps auto-catch new confirm structures. No new test files needed unless existing patterns require explicit coverage.

## Non-goals

- Undo/restore deleted content (tombstones are permanent from the user's perspective)
- Drag-and-drop reorder (arrow buttons only, matching OutlineEditor)
- Bulk rename
- Moving content between workspaces
- Archive/soft-delete with restore UI
