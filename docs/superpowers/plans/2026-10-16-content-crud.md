# Content CRUD Management Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use superpowers:subagent-driven-development (recommended) or superpowers:executing-plans to implement this plan task-by-task. Steps use checkbox (`- [ ]`) syntax for tracking.

**Goal:** Let users rename, delete, bulk-delete, and reorder their decks, quizzes, reports, and cheatsheets, from both list and detail screens.

**Architecture:** The backend and the frontend data layer are already written (uncommitted in the working tree). What remains is (a) the backend test suite that proves the new routes, (b) `docs/API.md` coverage, and (c) the entire UI layer: inline row rename/delete, detail-screen delete, bulk-select mode, and reorder controls.

**Tech Stack:** Rust (axum, sqlx, tokio) server; React 19 + TypeScript + Vite; TanStack Query v5; react-router; i18next.

## State of the working tree

Already implemented, **uncommitted** — verify, do not rewrite:

| Area | Where |
|---|---|
| `DELETE` + `PATCH` for decks, quizzes, reports, cheatsheets | `crates/server/src/{flashcards,quizzes,reports,cheatsheets}/routes.rs` |
| Workspace-delete cascade over all four content types | `crates/server/src/workspaces.rs:366-455` |
| `POST /workspaces/{id}/bulk-delete` | `crates/server/src/workspaces.rs:556` |
| `PATCH /workspaces/{id}/content-order` | `crates/server/src/workspaces.rs:689` |
| `sort_order` migration | `migrations/0041_*` |
| `route_sweep` + `tamper` classification rows | `crates/server/tests/route_sweep.rs:150-285`, `tamper.rs:1234-1266` |
| All eight frontend hooks | `app/src/api/queries/{quizzes,reports,cheatsheets,flashcards,workspaces}.ts` |
| Most delete/rename i18n keys | `app/src/i18n/locales/{en,id}.json` |

Missing: the backend tests, the API docs, and every piece of UI.

## Global Constraints

- Every user-visible string needs a key in **both** `app/src/i18n/locales/en.json` and `app/src/i18n/locales/id.json`. Plurals: English gets `_one`/`_other`, Indonesian `_other` only. `app/src/i18n/i18n.test.ts` asserts placeholders resolve.
- All deletion is a **tombstone** (`deleted_at = now()`), never a row delete. Nothing cascades automatically, so children are tombstoned by hand in the same transaction.
- Zeroing `credits_spent` on a `generating` row is mandatory in every delete path: `fail_and_refund` claims on `credits_spent > 0` with no `deleted_at` filter, so leaving it set lets a job refund a row the student deliberately deleted. A `failed` row's charge stays — it is a debt the boot sweep still owes.
- Destructive UI is an **in-row confirm block**, never a modal. There is no modal primitive and `WorkspaceListScreen.tsx:36-40` records the decision not to add one.
- Mutations **invalidate**, never seed the cache. No optimistic updates, no `removeQueries`.
- Errors render through `<ApiErrorNotice error={asApiError(mutation.error)} noun={t('errors.nouns.<type>')} />`. Branch on `code`, never on message text.
- Row layout uses `@container` queries (`@md:`), not viewport breakpoints. Titles keep `w-full min-w-0 wrap-anywhere font-medium @md:w-auto @md:flex-1`.
- There is **no `chevron-up` icon** (`app/src/components/ui/Icon.tsx` has `chevron-left`, `chevron-right`, `chevron-down`, `arrow-left`). Reorder controls are text-labelled buttons, matching `OutlineEditor` (`ReportScreen.tsx:470-490`).
- Never test wiring, forwarding, mock echoes, or source text. Tests assert behavior, boundaries, invariants, and errors.

## The i18n commit constraint

`app/src/i18n/i18n.test.ts` includes a test named `feature copy nothing renders` that fails when a locale key is defined but no component renders it. Twenty-seven of the keys already sitting uncommitted in the locale files are currently unused, because the UI that renders them does not exist yet.

Consequences for task order:

1. **Task 9 commits `app/src/api/queries/` only** — not the locale files.
2. **Tasks 10–15 commit only `.tsx` files.** The keys they need are already present in the working tree; each task's i18n step is there to add the handful that are still missing, not to re-add the rest. Leave both locale files uncommitted; `git status` showing them as modified throughout is expected and correct.
3. **Task 16 commits both locale files**, once every key has a renderer, and verifies the i18n suite goes green.

Committing the locales earlier would commit a red test suite. If a reviewer wants smaller commits, split the locale diff with `git add -p` so each task commits only the keys its own component renders.

## Exact hook signatures (already implemented — consume these, do not redefine)

| Hook | Call | Mutation argument |
|---|---|---|
| `useDeleteDeck(workspaceId)` | `flashcards.ts:168` | `deckId: string` |
| `useRenameDeck(workspaceId)` | `flashcards.ts:188` | `{ deckId, title }` |
| `useDeleteQuiz(workspaceId)` | `quizzes.ts:220` | `quizId: string` |
| `useRenameQuiz(workspaceId)` | `quizzes.ts:234` | `{ quizId, title }` |
| `useDeleteReport(workspaceId)` | `reports.ts:288` | `reportId: string` |
| `useCheatsheets(workspaceId)` | `cheatsheets.ts:72` | — (query) |
| `useDeleteCheatsheet(workspaceId)` | `cheatsheets.ts:197` | `cheatsheetId: string` |
| `useRenameCheatsheet(workspaceId)` | `cheatsheets.ts:211` | `{ cheatsheetId, title }` |
| `useBulkDelete(workspaceId)` | `workspaces.ts:148` | `ContentIds` |
| `useReorderContent(workspaceId)` | `workspaces.ts:167` | `ContentIds` |

`ContentIds` (`workspaces.ts:126-131`) is `{ deckIds?, quizIds?, reportIds?, cheatsheetIds? }`, all optional.

---

### Task 1: Quiz delete and rename tests

**Files:**
- Modify: `crates/server/tests/quizzes.rs` (append at end; helpers at `:55-140`)

**Interfaces:**
- Consumes: `quiz_created(app, workspace_id, token, source_id, count) -> (StatusCode, Value)` (`:56`), `generated_quiz(app, harness, token, workspace_id, source_id) -> (String, Vec<String>)` (`:77`), `answered(app, token, quiz_id, question_id, answer, correct) -> Value` (`:104`), `common::{drain_jobs, json, signed_up, workspace_with_text}`.
- Produces: test names only.

- [ ] **Step 1: Write the failing test**

Append to `crates/server/tests/quizzes.rs` (add `use uuid::Uuid;` to the imports if absent):

```rust
/// Deleting a quiz tombstones it and its questions, and the attempt history survives.
///
/// The attempts are the point: `quiz_attempts` has no `deleted_at` column, so a delete
/// cannot reach them, and that is the intended contract — the dashboard's "quizzes taken"
/// figure counts distinct quizzes with an attempt in the window, and a student who deletes
/// a quiz has still done the work. A delete that also removed attempts would silently
/// rewrite a figure the student already saw.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deleted_quiz_is_gone_from_the_list_and_its_questions_are_tombstoned(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "quiz-deleter@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, question_ids) =
        generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;
    answered(&app, &token, &quiz_id, &question_ids[0], "Membuka koneksi TCP.", None).await;

    let (status, body) = json(
        &app,
        "DELETE",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (_, list) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/quizzes"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(list["quizzes"].as_array().unwrap().len(), 0, "{list}");

    // Gone by id, and a second delete is 404 like the first never happened.
    let (status, _) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    // The questions are tombstoned as rows, and the attempt survives.
    let quiz_uuid = Uuid::parse_str(&quiz_id).unwrap();
    let live_questions: i64 = sqlx::query_scalar(
        "select count(*) from quiz_questions where quiz_id = $1 and deleted_at is null",
    )
    .bind(quiz_uuid)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_questions, 0, "the questions must be tombstoned with the quiz");

    let attempts: i64 = sqlx::query_scalar("select count(*) from quiz_attempts where quiz_id = $1")
        .bind(quiz_uuid)
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    assert_eq!(attempts, 1, "attempt history outlives the quiz");
}

/// Renaming a quiz changes only its title.
#[sqlx::test(migrations = "../../migrations")]
async fn a_quiz_can_be_renamed(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![reply(1)])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "quiz-renamer@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (quiz_id, _) = generated_quiz(&app, &harness, &token, &workspace_id, &source_id).await;

    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::json!({ "title": "  UTS Jaringan  " }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["quiz"]["title"], "UTS Jaringan", "the title is trimmed: {body}");
    assert_eq!(body["quiz"]["question_count"], 2, "renaming must not touch questions: {body}");

    let (status, _) = json(
        &app,
        "PATCH",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::json!({ "title": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, quiz) = json(
        &app,
        "GET",
        &format!("/quizzes/{quiz_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(quiz["quiz"]["title"], "UTS Jaringan");
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --test quizzes a_deleted_quiz_is_gone a_quiz_can_be_renamed`
Expected: both PASS — `delete_quiz` and `rename_quiz` already exist.

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/quizzes.rs
git commit -m "test(quizzes): cover delete, tombstoned questions, surviving attempts, and rename"
```

---

### Task 2: Report delete and rename tests

**Files:**
- Modify: `crates/server/tests/reports.rs` (append at end)

**Interfaces:**
- Consumes: `report_created(app, workspace_id, token, source_id) -> (StatusCode, Value)` (`:45` — **workspace id before token**), `get_report(app, token, report_id) -> Value` (`:66`, returns the whole `{ "report": … }` envelope).
- Produces: test names only.

- [ ] **Step 1: Write the failing test**

```rust
/// Deleting a report tombstones it, and both routes that reach it stop answering.
///
/// A report is one row — the outline and the section texts both live on it — so there is no
/// child to tombstone and nothing to leak. What the test pins is that the row stops being
/// readable through the list *and* through the by-id read, because the list is what the
/// Studio panel polls and the by-id read is what the report screen renders.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deleted_report_is_gone_from_the_list(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // `TestState::new` installs no provider, and `report_created` posts to
    // `/reports/outline`, which is a synchronous model call. Without a scripted reply
    // that route answers `provider_error` and the create assertion fails.
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![outline_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "report-deleter@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (status, created) = report_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let report_id = created["report"]["id"].as_str().unwrap().to_string();

    let (status, body) = json(
        &app,
        "DELETE",
        &format!("/reports/{report_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (_, list) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/reports"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(list["reports"].as_array().unwrap().len(), 0, "{list}");

    let (status, _) = json(
        &app,
        "GET",
        &format!("/reports/{report_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/reports/{report_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);
}

/// Renaming a report is an outline edit: the title is the outline's.
///
/// `PATCH /reports/:id` replaces the whole outline, so a rename is that same call with a
/// changed `title` and every section otherwise untouched — which is also what makes the
/// section texts survive it, because the server matches each section's text to its heading.
#[sqlx::test(migrations = "../../migrations")]
async fn a_report_title_is_edited_through_the_outline(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // Same reason as the delete test above.
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![outline_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "report-renamer@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (status, created) = report_created(&app, &workspace_id, &token, &source_id).await;
    assert_eq!(status, StatusCode::CREATED, "{created}");
    let report_id = created["report"]["id"].as_str().unwrap().to_string();

    let before = get_report(&app, &token, &report_id).await;
    let sections = before["report"]["outline"]["sections"].as_array().unwrap().len();
    let mut renamed = before["report"]["outline"].clone();
    renamed["title"] = serde_json::json!("UTS Jaringan");

    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/reports/{report_id}"),
        &token,
        serde_json::json!({ "outline": renamed }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["report"]["title"], "UTS Jaringan", "{body}");

    let after = get_report(&app, &token, &report_id).await;
    assert_eq!(after["report"]["title"], "UTS Jaringan");
    assert_eq!(
        after["report"]["outline"]["sections"].as_array().unwrap().len(),
        sections,
        "a rename must not drop sections: {after}"
    );
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --test reports a_deleted_report_is_gone a_report_title_is_edited`
Expected: both PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/reports.rs
git commit -m "test(reports): cover delete and title edit through the outline"
```

---

### Task 3: Cheatsheet delete and rename tests

**Files:**
- Modify: `crates/server/tests/cheatsheets.rs` (append at end)

**Interfaces:**
- Consumes: `generated(app, harness, token, workspace_id, source_id) -> String` (`:98`), `get_cheatsheet(app, token, id) -> Value` (`:85`).
- Produces: test names only.

- [ ] **Step 1: Write the failing test**

Add `use uuid::Uuid;` to the imports if absent, then append:

```rust
/// Deleting a cheatsheet tombstones the row and leaves the stored objects alone.
///
/// The three `*_key` columns point into the object bucket, and a tombstone does not touch
/// them — the account purge removes the bytes, and it already walks those columns. Asserting
/// the keys survive is the point: a delete that nulled them would orphan the object forever,
/// because nothing else records its name.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deleted_cheatsheet_is_gone_from_the_list_and_keeps_its_objects(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // `TestState::new` installs no provider, and `generated` drains a real generation
    // job. Without a scripted reply the job fails, the object keys stay NULL, and this
    // test's central assertion — that the keys survive the tombstone — would fail for a
    // reason that has nothing to do with deletion.
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![blocks_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "sheet-deleter@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let cheatsheet_id = generated(&app, &harness, &token, &workspace_id, &source_id).await;

    let (status, body) = json(
        &app,
        "DELETE",
        &format!("/cheatsheets/{cheatsheet_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (_, list) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace_id}/cheatsheets"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(list["cheatsheets"].as_array().unwrap().len(), 0, "{list}");

    let (status, _) = json(
        &app,
        "GET",
        &format!("/cheatsheets/{cheatsheet_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let (status, _) = json(
        &app,
        "DELETE",
        &format!("/cheatsheets/{cheatsheet_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND);

    let keys: (Option<String>, Option<String>, Option<String>) =
        sqlx::query_as("select blocks_key, html_key, pdf_key from cheatsheets where id = $1")
            .bind(Uuid::parse_str(&cheatsheet_id).unwrap())
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert!(
        keys.0.is_some() && keys.1.is_some(),
        "the stored object keys must survive a tombstone, or the bytes are orphaned: {keys:?}"
    );
}

/// Renaming a cheatsheet changes its title, not its scope.
#[sqlx::test(migrations = "../../migrations")]
async fn a_cheatsheet_can_be_renamed(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    // Same reason as the delete test above: `generated` needs a scripted reply.
    let harness = harness.with_llm(Arc::new(ScriptedLlm::new(vec![blocks_reply()])));
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "sheet-renamer@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let cheatsheet_id = generated(&app, &harness, &token, &workspace_id, &source_id).await;
    let before = get_cheatsheet(&app, &token, &cheatsheet_id).await;
    let scope = before["cheatsheet"]["scope"].clone();

    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/cheatsheets/{cheatsheet_id}"),
        &token,
        serde_json::json!({ "title": "  Rangkuman UTS  " }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["cheatsheet"]["title"], "Rangkuman UTS", "{body}");
    assert_eq!(body["cheatsheet"]["scope"], scope, "the scope is not the title: {body}");

    let (status, _) = json(
        &app,
        "PATCH",
        &format!("/cheatsheets/{cheatsheet_id}"),
        &token,
        serde_json::json!({ "title": "" }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let after = get_cheatsheet(&app, &token, &cheatsheet_id).await;
    assert_eq!(after["cheatsheet"]["title"], "Rangkuman UTS");
}
```

- [ ] **Step 2: Run the tests**

Run: `cargo test --test cheatsheets a_deleted_cheatsheet_is_gone a_cheatsheet_can_be_renamed`
Expected: both PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/cheatsheets.rs
git commit -m "test(cheatsheets): cover delete (objects survive) and rename"
```

---

### Task 4: Deck rename test

**Files:**
- Modify: `crates/server/tests/flashcards.rs` (append at end)

**Interfaces:**
- Consumes: `deck_created(app, workspace_id, token, source_id, count) -> (StatusCode, Value)` (`:29`), `reply(front, back, label) -> String` (`:25`), `drain_jobs(&harness)`.
- Produces: test names only.

- [ ] **Step 1: Write the failing test**

```rust
/// A deck can be renamed without touching its cards or its status.
#[sqlx::test(migrations = "../../migrations")]
async fn a_deck_can_be_renamed(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Apa itu TCP?", "Protokol.", 1)]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "deck-renamer@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::json!({ "title": "  UTS Jaringan  " }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{body}");
    assert_eq!(body["deck"]["title"], "UTS Jaringan", "the title is trimmed: {body}");
    assert_eq!(body["deck"]["card_count"], 1, "renaming must not touch the cards: {body}");
    assert_eq!(body["deck"]["status"], "ready", "{body}");

    let (status, _) = json(
        &app,
        "PATCH",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::json!({ "title": "   " }),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);

    let (_, deck) = json(
        &app,
        "GET",
        &format!("/decks/{deck_id}"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    assert_eq!(deck["deck"]["title"], "UTS Jaringan");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --test flashcards a_deck_can_be_renamed`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/flashcards.rs
git commit -m "test(flashcards): cover deck rename"
```

---

### Task 5: Workspace-delete cascade test

**Files:**
- Modify: `crates/server/tests/devices_workspaces.rs` (append at end)

**Interfaces:**
- Consumes: the file's own helpers — `signup`, `token_of`, `create_workspace`, and `delete(app, uri, token) -> (StatusCode, Value)` at `:79`. Read `deleting_a_workspace_tombstones_its_sources` (`:561`) for the exact call shapes before writing this.
- Produces: test names only.

- [ ] **Step 1: Write the failing test**

Insert rows with `sqlx` rather than driving four generators: this test is about the cascade, and four scripted model replies would bury the assertion.

```rust
/// Deleting a workspace tombstones every content row it owns, in the same transaction.
///
/// This is not tidiness. The entitlement counters filter on each content table's **own**
/// `deleted_at`, not the workspace's, so a deck left live under a deleted workspace keeps
/// counting against the student's monthly limit while being unreachable — every list route
/// calls `owned_name` first. The student would be charged for content they cannot open and
/// cannot delete, because the workspace that owned it is gone.
#[sqlx::test(migrations = "../../migrations")]
async fn deleting_a_workspace_tombstones_its_content(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "content-cascade@example.test", "Laptop").await;
    let token = token_of(&account);
    let workspace = create_workspace(&app, &token, "Cascade").await;

    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind("content-cascade@example.test")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let workspace_id = Uuid::parse_str(&workspace).unwrap();

    // One live row per content type. The column lists differ because the required
    // columns do: `decks.source_ids` has no default, and `cheatsheets.scope` is
    // `not null` with no default either.
    sqlx::query(
        "insert into decks (user_id, workspace_id, title, source_ids, status)
         values ($1, $2, 'live', array[]::uuid[], 'ready')",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "insert into quizzes (user_id, workspace_id, title, status)
         values ($1, $2, 'live', 'ready')",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "insert into reports (user_id, workspace_id, title, status)
         values ($1, $2, 'live', 'ready')",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    sqlx::query(
        "insert into cheatsheets (user_id, workspace_id, title, scope, status)
         values ($1, $2, 'live', 'UTS Jaringan', 'ready')",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    // A card under the deck, so the child sweep is exercised too.
    sqlx::query(
        "insert into flashcards (user_id, deck_id, front, back)
         select $1, id, 'front', 'back' from decks where workspace_id = $2",
    )
    .bind(user_id)
    .bind(workspace_id)
    .execute(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = delete(&app, &format!("/workspaces/{workspace}"), &token).await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    // Four literal statements rather than one `format!` loop: `query_scalar` takes
    // `impl SqlSafeStr`, which only `&'static str` implements, so a table name
    // interpolated at runtime is a compile error by design.
    let live_decks: i64 = sqlx::query_scalar(
        "select count(*) from decks where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_decks, 0, "decks must be tombstoned with the workspace");

    let live_quizzes: i64 = sqlx::query_scalar(
        "select count(*) from quizzes where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_quizzes, 0, "quizzes must be tombstoned with the workspace");

    let live_reports: i64 = sqlx::query_scalar(
        "select count(*) from reports where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_reports, 0, "reports must be tombstoned with the workspace");

    let live_cheatsheets: i64 = sqlx::query_scalar(
        "select count(*) from cheatsheets where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(
        live_cheatsheets, 0,
        "cheatsheets must be tombstoned with the workspace"
    );

    let live_cards: i64 = sqlx::query_scalar(
        "select count(*) from flashcards f
           join decks d on d.id = f.deck_id
          where d.workspace_id = $1 and f.deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_cards, 0, "a deck's cards go with the deck");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --test devices_workspaces deleting_a_workspace_tombstones_its_content`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/devices_workspaces.rs
git commit -m "test(workspaces): deleting a workspace tombstones all its content"
```

---

### Task 6: Bulk-delete test

**Files:**
- Modify: `crates/server/tests/devices_workspaces.rs` (append at end)

**Interfaces:**
- Consumes: `signup`, `token_of`, `create_workspace`, and `common::json` for the POST.
- Produces: test names only.

- [ ] **Step 1: Write the failing test**

```rust
/// Bulk delete tombstones every id in one transaction, and skips ids that are not the
/// workspace's.
///
/// The skip is the contract, not an oversight: a bulk delete runs against a list the client
/// rendered, and a row another device already deleted is a normal race, not an error worth
/// showing. What must not happen is the whole batch failing over one stale id — that would
/// leave the other items undeleted and the button apparently broken.
#[sqlx::test(migrations = "../../migrations")]
async fn bulk_delete_tombstones_the_batch_and_skips_stale_ids(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "bulk@example.test", "Laptop").await;
    let token = token_of(&account);
    let workspace = create_workspace(&app, &token, "Bulk").await;

    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind("bulk@example.test")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let workspace_id = Uuid::parse_str(&workspace).unwrap();

    let mut deck_ids = Vec::new();
    for index in 0..3 {
        let id: Uuid = sqlx::query_scalar(
            "insert into decks (user_id, workspace_id, title, source_ids, status)
             values ($1, $2, $3, array[]::uuid[], 'ready') returning id",
        )
        .bind(user_id)
        .bind(workspace_id)
        .bind(format!("deck {index}"))
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        deck_ids.push(id);
    }
    let quiz_id: Uuid = sqlx::query_scalar(
        "insert into quizzes (user_id, workspace_id, title, source_ids, status)
         values ($1, $2, 'quiz', array[]::uuid[], 'ready') returning id",
    )
    .bind(user_id)
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    // A deck in *another* workspace of the same account: it must survive, because the
    // route's guard is the workspace as well as the user.
    let other_workspace = create_workspace(&app, &token, "Other").await;
    let other_id: Uuid = sqlx::query_scalar(
        "insert into decks (user_id, workspace_id, title, source_ids, status)
         values ($1, $2, 'elsewhere', array[]::uuid[], 'ready') returning id",
    )
    .bind(user_id)
    .bind(Uuid::parse_str(&other_workspace).unwrap())
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/bulk-delete"),
        &token,
        serde_json::json!({
            "deck_ids": [
                deck_ids[0].to_string(),
                deck_ids[1].to_string(),
                // A nil uuid that matches no row, and a real id in another workspace:
                // both are skipped rather than failing the batch.
                Uuid::nil().to_string(),
                other_id.to_string(),
            ],
            "quiz_ids": [quiz_id.to_string()],
        }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let live_decks: i64 = sqlx::query_scalar(
        "select count(*) from decks where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_decks, 1, "the third deck was not in the batch");

    let other_live: i64 =
        sqlx::query_scalar("select count(*) from decks where id = $1 and deleted_at is null")
            .bind(other_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    assert_eq!(other_live, 1, "another workspace's deck must not be reachable");

    let live_quizzes: i64 = sqlx::query_scalar(
        "select count(*) from quizzes where workspace_id = $1 and deleted_at is null",
    )
    .bind(workspace_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(live_quizzes, 0);

    // An empty batch is a client bug, not a no-op.
    let (status, _) = json(
        &app,
        "POST",
        &format!("/workspaces/{workspace}/bulk-delete"),
        &token,
        serde_json::json!({}),
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --test devices_workspaces bulk_delete_tombstones_the_batch`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/devices_workspaces.rs
git commit -m "test(workspaces): bulk delete tombstones the batch and skips stale ids"
```

---

### Task 7: Content-reorder test

**Files:**
- Modify: `crates/server/tests/devices_workspaces.rs` (append at end)

**Interfaces:**
- Consumes: same helpers as Task 6.
- Produces: test names only.

- [ ] **Step 1: Write the failing test**

```rust
/// `PATCH /workspaces/:id/content-order` sets `sort_order` from the array index, and the
/// list route honours it.
///
/// Ordering by `created_at desc, id` was the old behaviour, and it is wrong for this feature
/// for a specific reason: `created_at` defaults to `now()`, and a generation that inserts
/// several rows in one transaction gives them the same microsecond, so the tiebreak fell
/// through to a random uuid. A student who reorders a list must see the order they chose.
#[sqlx::test(migrations = "../../migrations")]
async fn content_order_sets_the_list_order(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let account = signup(&app, &harness, "reorder@example.test", "Laptop").await;
    let token = token_of(&account);
    let workspace = create_workspace(&app, &token, "Order").await;
    let user_id: Uuid = sqlx::query_scalar("select id from users where email = $1")
        .bind("reorder@example.test")
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
    let workspace_id = Uuid::parse_str(&workspace).unwrap();

    let mut ids = Vec::new();
    for index in 0..3 {
        let id: Uuid = sqlx::query_scalar(
            "insert into decks (user_id, workspace_id, title, source_ids, status)
             values ($1, $2, $3, array[]::uuid[], 'ready') returning id",
        )
        .bind(user_id)
        .bind(workspace_id)
        .bind(format!("deck {index}"))
        .fetch_one(&harness.state.pool)
        .await
        .unwrap();
        ids.push(id);
    }

    // Reverse the creation order.
    let reversed: Vec<String> = ids.iter().rev().map(Uuid::to_string).collect();
    let (status, body) = json(
        &app,
        "PATCH",
        &format!("/workspaces/{workspace}/content-order"),
        &token,
        serde_json::json!({ "deck_ids": reversed }),
    )
    .await;
    assert_eq!(status, StatusCode::NO_CONTENT, "{body}");

    let (_, list) = json(
        &app,
        "GET",
        &format!("/workspaces/{workspace}/decks"),
        &token,
        serde_json::Value::Null,
    )
    .await;
    let titles: Vec<&str> = list["decks"]
        .as_array()
        .unwrap()
        .iter()
        .map(|deck| deck["title"].as_str().unwrap())
        .collect();
    assert_eq!(titles, ["deck 2", "deck 1", "deck 0"], "{list}");
}
```

- [ ] **Step 2: Run the test**

Run: `cargo test --test devices_workspaces content_order_sets_the_list_order`
Expected: PASS.

- [ ] **Step 3: Commit**

```bash
git add crates/server/tests/devices_workspaces.rs
git commit -m "test(workspaces): content order sets the list order"
```

---

### Task 8: API documentation

**Files:**
- Modify: `docs/API.md` (the **Read-only** classification table around `:1079-1110`, plus the route prose sections)

**Interfaces:**
- Consumes: nothing.
- Produces: nothing importable.

- [ ] **Step 1: Add the read-only table rows**

In the **Read-only** table, alongside the existing `DELETE /decks/{id}` row, add rows for the routes the router now serves but the table does not name:

| Method | Path | Why it is not swept |
|---|---|---|
| PATCH | `/decks/{id}` | renaming a deck spends nothing |
| PATCH | `/quizzes/{id}` | renaming a quiz spends nothing |
| DELETE | `/quizzes/{id}` | soft-deletes the caller's own quiz and its questions; the attempt history stays, because the dashboard's "quizzes taken" counts work already done |
| DELETE | `/reports/{id}` | soft-deletes the caller's own report |
| PATCH | `/cheatsheets/{id}` | renaming a cheatsheet spends nothing |
| DELETE | `/cheatsheets/{id}` | soft-deletes the caller's own cheatsheet; its stored objects are reclaimed by the account purge |
| POST | `/workspaces/{id}/bulk-delete` | deleting content frees the monthly counters the refusals name |
| PATCH | `/workspaces/{id}/content-order` | reordering a list spends nothing |

Where a method/path pair already has a row, extend that row's reason instead of adding a duplicate.

- [ ] **Step 2: Add the prose sections**

Find the prose section documenting `DELETE /decks/{id}` and add sibling subsections for each new route, stating: the method and path, the 204 response, that the delete is a tombstone, the credit-zeroing rule for `generating` rows, and — for bulk delete — the skip-stale-ids contract and the 400 on an empty batch.

- [ ] **Step 3: Verify against the router**

Run: `cargo test --test route_sweep every_registered_route_is_classified`
Expected: PASS. Then read the new rows against `crates/server/src/{flashcards,quizzes,reports,cheatsheets}/routes.rs` and `crates/server/src/workspaces.rs:25-45`, confirming every registered route is listed with the same method.

- [ ] **Step 4: Commit**

```bash
git add docs/API.md
git commit -m "docs(api): document the content delete, rename, bulk-delete and reorder routes"
```

---

### Task 9: Commit the existing query hooks

**Files:** none modified — this task commits work already in the tree.

**Interfaces:**
- Consumes: the uncommitted hooks in `app/src/api/queries/`.
- Produces: a clean baseline for Tasks 10–16.

**Do not commit `app/src/i18n/locales/` in this task.** `app/src/i18n/i18n.test.ts` has a test named `feature copy nothing renders` that fails when a locale key is defined but no component renders it. Twenty-seven of the keys currently sitting uncommitted in the locale files — every `studio.quiz.delete*`, `studio.report.delete*`, `studio.cheatsheet.delete*`, `studio.deck.rename` and `studio.bulk.*` entry — are unused, because none of the UI that renders them exists yet. Committing the locales here would commit a red test. The locale files are committed once, at the end of Task 16, after every one of those keys has a renderer.

Run this to see the constraint for yourself:

```bash
npm --prefix app test -- i18n
```

Expected right now: 1 failure listing the 27 unused keys. Expected after Task 16: 10 passed.

- [ ] **Step 1: Verify the hooks type-check**

Run: `npm --prefix app run build`
Expected: no TypeScript errors. Verified clean as of this writing. If a hook does not compile, fix it here — every later task depends on these signatures.

- [ ] **Step 2: Commit the hooks only**

```bash
git add app/src/api/queries/
git commit -m "feat(web): delete, rename, bulk-delete and reorder query hooks"
```

- [ ] **Step 3: Confirm the locales are still uncommitted**

Run: `git status --short app/src/i18n/locales/`
Expected: ` M app/src/i18n/locales/en.json` and ` M app/src/i18n/locales/id.json` — still modified, still uncommitted. That is correct.

---

### Task 10: Quiz row — delete and rename

**Files:**
- Modify: `app/src/features/studio/QuizPanel.tsx` (the `QuizRow` function at `:196-230`, and its call site in `QuizPanel`)
- Modify: `app/src/i18n/locales/en.json`, `app/src/i18n/locales/id.json` (inside `studio.quiz`)

**Interfaces:**
- Consumes: `useDeleteQuiz(workspaceId)` → `mutate(quizId)`; `useRenameQuiz(workspaceId)` → `mutate({ quizId, title })`.
- Produces: `QuizRow` with a `workspaceId` prop — consumed by Task 15's select mode.

- [ ] **Step 1: Add the three missing i18n keys**

`studio.quiz` already has `deleteConfirm`, `deleteIt`, `deleteNoQuestions`, `deleteQuestions_one/_other`, `deleting`, `rename` in both locales. Add only what is missing — the form's label and its button text. The buttons themselves reuse `actions.save` and `actions.cancel`, which already exist, so no per-type `save`/`saving` key is needed:

`en.json`:
```json
      "renameLabel": "Quiz name"
```
`id.json`:
```json
      "renameLabel": "Nama kuis"
```

- [ ] **Step 2: Replace `QuizRow`**

Replace the whole function. It currently takes `{ quiz }: { quiz: Quiz }`; it now also needs `workspaceId` for the hooks:

```tsx
function QuizRow({ quiz, workspaceId }: { quiz: Quiz; workspaceId: string }) {
  const { t } = useTranslation()
  const remove = useDeleteQuiz(workspaceId)
  const rename = useRenameQuiz(workspaceId)
  const [mode, setMode] = useState<'idle' | 'renaming' | 'deleting'>('idle')
  const removeError = asApiError(remove.error)
  const renameError = asApiError(rename.error)
  // Resolved here rather than at module scope: a constant holding copy is
  // evaluated before there is a language, so it could only ever ship one of them.
  const statusLabel: Record<QuizStatus, string> = {
    generating: t('studio.status.generating'),
    ready: t('studio.status.ready'),
    failed: t('studio.status.failed'),
  }
  const displayName = quiz.title.length > 0 ? quiz.title : t('studio.quiz.untitled')

  return (
    <li className="flex flex-col gap-2 py-3">
      <div className="flex flex-wrap items-center gap-x-3 gap-y-2">
        {/* `w-full … @md:w-auto @md:flex-1` is load-bearing: the other children are
            effectively fixed-width (a count, a status chip, links) while `flex-1`
            gives this span a flex basis of `0`, so without it the title is squeezed
            to its min-content — one character — and a long Indonesian word renders
            one character per line. The condition is the *panel's* width, not the
            window's, so the breakpoint is a container one. */}
        <span className="w-full min-w-0 wrap-anywhere font-medium @md:w-auto @md:flex-1">
          {displayName}
        </span>
        <span className="meta">{t('studio.quiz.questions', { count: quiz.question_count })}</span>
        <span className={STATUS_CHIP[quiz.status]} role="status">
          {quiz.status === 'generating' && (
            <span className="h-2 w-2 animate-working rounded-full bg-current" aria-hidden="true" />
          )}
          {statusLabel[quiz.status]}
        </span>
        {quiz.status === 'ready' && (
          <Link to={`/quiz/${quiz.id}`} className="btn btn-secondary">
            {t('studio.quiz.take')}
          </Link>
        )}
        {mode === 'idle' && (
          <>
            <button
              type="button"
              className="btn btn-quiet text-sm"
              onClick={() => { setMode('renaming') }}
            >
              {t('studio.quiz.rename')}
            </button>
            <button
              type="button"
              className="btn btn-quiet text-danger text-sm"
              onClick={() => { setMode('deleting') }}
            >
              {t('actions.delete')}
            </button>
          </>
        )}
      </div>

      {mode === 'renaming' && (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            const title = event.currentTarget.elements.namedItem('quiz-title')
            if (!(title instanceof HTMLInputElement)) return
            rename.mutate(
              { quizId: quiz.id, title: title.value },
              { onSuccess: () => { setMode('idle') } },
            )
          }}
        >
          <label className="flex min-w-0 flex-1 flex-col gap-1">
            <span className="text-small">{t('studio.quiz.renameLabel')}</span>
            <input
              name="quiz-title"
              className="field"
              defaultValue={quiz.title}
              autoFocus
              disabled={rename.isPending}
            />
          </label>
          <button type="submit" className="btn btn-primary text-sm" disabled={rename.isPending}>
            {t('actions.save')}
          </button>
          <button
            type="button"
            className="btn btn-quiet text-sm"
            onClick={() => {
              rename.reset()
              setMode('idle')
            }}
          >
            {t('actions.cancel')}
          </button>
        </form>
      )}

      {mode === 'deleting' && (
        <div className="note note-danger flex flex-col items-start gap-2">
          <p className="font-medium">
            {t('studio.quiz.deleteConfirm', { name: displayName })}
          </p>
          <p className="text-small">
            {quiz.question_count === 0
              ? t('studio.quiz.deleteNoQuestions')
              : t('studio.quiz.deleteQuestions', { count: quiz.question_count })}
          </p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-secondary"
              autoFocus
              disabled={remove.isPending}
              onClick={() => {
                remove.mutate(quiz.id, {
                  onSuccess: () => { setMode('idle') },
                })
              }}
            >
              {remove.isPending ? t('studio.quiz.deleting') : t('studio.quiz.deleteIt')}
            </button>
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => {
                remove.reset()
                setMode('idle')
              }}
            >
              {t('actions.cancel')}
            </button>
          </div>
        </div>
      )}

      {removeError !== null && (
        <ApiErrorNotice error={removeError} noun={t('errors.nouns.quiz')} />
      )}
      {renameError !== null && (
        <ApiErrorNotice error={renameError} noun={t('errors.nouns.quiz')} />
      )}

      {quiz.error !== null && quiz.error.length > 0 && (
        <p className="note note-danger text-sm">{quiz.error}</p>
      )}
    </li>
  )
}
```

- [ ] **Step 3: Pass `workspaceId` at the call site**

In `QuizPanel`, change `<QuizRow key={quiz.id} quiz={quiz} />` to `<QuizRow key={quiz.id} quiz={quiz} workspaceId={workspaceId} />`. Add `useDeleteQuiz, useRenameQuiz` to the existing `@/api/queries/quizzes` import block.

- [ ] **Step 4: Run the frontend tests**

Run: `npm --prefix app test -- QuizPanel`
Expected: PASS. If a colocated `QuizPanel.test.tsx` asserts the old markup, update it to the new accessible names — a delete trigger named "Delete" and a rename trigger named "Rename" — rather than re-pinning markup.

- [ ] **Step 5: Commit**

```bash
git add app/src/features/studio/QuizPanel.tsx
git commit -m "feat(web): delete and rename a quiz from its row"
```

---

### Task 11: Report row — delete and rename

**Files:**
- Modify: `app/src/features/studio/ReportPanel.tsx` (the `ReportRow` function at `:311-375`)
- Modify: `app/src/i18n/locales/en.json`, `app/src/i18n/locales/id.json` (inside `studio.report`)

**Interfaces:**
- Consumes: `useDeleteReport(workspaceId)` → `mutate(reportId)`; `useUpdateOutline(reportId)` → `mutate(ReportOutline)` (`reports.ts:227` — a report's title is its outline's, so this *is* the rename).
- Produces: `ReportRow` unchanged in signature.

- [ ] **Step 1: Add the two missing i18n keys**

`studio.report` already has `deleteConfirm`, `deleteIt`, `deleteNoSections`, `deleteSections_one/_other`, `deleting`. Add:

`en.json`:
```json
      "rename": "Rename",
      "renameLabel": "Report title"
```
`id.json`:
```json
      "rename": "Ganti nama",
      "renameLabel": "Judul laporan"
```

- [ ] **Step 2: Add the state and derived names**

`ReportRow` already computes `total` and `written`. Add:

```tsx
  const remove = useDeleteReport(report.workspace_id)
  const rename = useUpdateOutline(report.id)
  const [mode, setMode] = useState<'idle' | 'renaming' | 'deleting'>('idle')
  const removeError = asApiError(remove.error)
  const renameError = asApiError(rename.error)
  const displayName = report.title.length > 0 ? report.title : t('studio.report.untitled')
```

Replace the row's inline `{report.title.length > 0 ? report.title : t('studio.report.untitled')}` with `{displayName}`.

- [ ] **Step 3: Add the triggers after the `Open` link**

```tsx
        {mode === 'idle' && (
          <>
            {/* A report's title *is* its outline's, so a report with no outline has no
                title to rename. The trigger is omitted rather than opening a form whose
                submit the server would refuse. */}
            {report.outline !== null && (
              <button
                type="button"
                className="btn btn-quiet text-sm"
                onClick={() => { setMode('renaming') }}
              >
                {t('studio.report.rename')}
              </button>
            )}
            <button
              type="button"
              className="btn btn-quiet text-danger text-sm"
              onClick={() => { setMode('deleting') }}
            >
              {t('actions.delete')}
            </button>
          </>
        )}
```

- [ ] **Step 4: Add the rename form**

`report.outline` is `ReportOutline | null` (`reports.ts:49-52`), so the `!== null` guard narrows it and the spread is type-safe:

```tsx
      {mode === 'renaming' && report.outline !== null && (
        <form
          className="flex flex-wrap items-end gap-2"
          onSubmit={(event) => {
            event.preventDefault()
            const title = event.currentTarget.elements.namedItem('report-title')
            if (!(title instanceof HTMLInputElement)) return
            // A report's title is its outline's, so a rename is the outline edit with
            // only the title changed. The sections go back exactly as they came: the
            // server matches each section's text to its heading, and a rename must not
            // lose work the student paid for.
            rename.mutate(
              { ...report.outline, title: title.value },
              { onSuccess: () => { setMode('idle') } },
            )
          }}
        >
          <label className="flex min-w-0 flex-1 flex-col gap-1">
            <span className="text-small">{t('studio.report.renameLabel')}</span>
            <input
              name="report-title"
              className="field"
              defaultValue={report.outline.title}
              autoFocus
              disabled={rename.isPending}
            />
          </label>
          <button type="submit" className="btn btn-primary text-sm" disabled={rename.isPending}>
            {t('actions.save')}
          </button>
          <button
            type="button"
            className="btn btn-quiet text-sm"
            onClick={() => {
              rename.reset()
              setMode('idle')
            }}
          >
            {t('actions.cancel')}
          </button>
        </form>
      )}
```

- [ ] **Step 5: Add the delete confirm and the error notices**

```tsx
      {mode === 'deleting' && (
        <div className="note note-danger flex flex-col items-start gap-2">
          <p className="font-medium">
            {t('studio.report.deleteConfirm', { name: displayName })}
          </p>
          <p className="text-small">
            {written === 0
              ? t('studio.report.deleteNoSections')
              : t('studio.report.deleteSections', { count: written })}
          </p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-secondary"
              autoFocus
              disabled={remove.isPending}
              onClick={() => {
                remove.mutate(report.id, {
                  onSuccess: () => { setMode('idle') },
                })
              }}
            >
              {remove.isPending ? t('studio.report.deleting') : t('studio.report.deleteIt')}
            </button>
            <button
              type="button"
              className="btn btn-quiet"
              onClick={() => {
                remove.reset()
                setMode('idle')
              }}
            >
              {t('actions.cancel')}
            </button>
          </div>
        </div>
      )}

      {removeError !== null && (
        <ApiErrorNotice error={removeError} noun={t('errors.nouns.report')} />
      )}
      {renameError !== null && (
        <ApiErrorNotice error={renameError} noun={t('errors.nouns.report')} />
      )}
```

Add `useDeleteReport` and `useUpdateOutline` to the `@/api/queries/reports` import block.

- [ ] **Step 6: Run the frontend tests**

Run: `npm --prefix app test -- ReportPanel`
Expected: PASS.

- [ ] **Step 7: Commit**

```bash
git add app/src/features/studio/ReportPanel.tsx
git commit -m "feat(web): delete and rename a report from its row"
```

---

### Task 12: Cheatsheet list with delete and rename

**Files:**
- Modify: `app/src/features/studio/ReportPanel.tsx` (add a `CheatsheetList` beside `CheatsheetForm`)
- Modify: `app/src/i18n/locales/en.json`, `app/src/i18n/locales/id.json` (inside `studio.cheatsheet`)

**Interfaces:**
- Consumes: `useCheatsheets(workspaceId)` (query), `useDeleteCheatsheet(workspaceId)` → `mutate(cheatsheetId)`, `useRenameCheatsheet(workspaceId)` → `mutate({ cheatsheetId, title })`.
- Produces: `CheatsheetList` / `CheatsheetRow` in `ReportPanel.tsx` — consumed by Task 15's select mode.

**Why this task exists:** `GET /workspaces/:id/cheatsheets` has been served since the list route was added, but nothing in the UI consumed it — a cheatsheet was reachable only through the id in its create response. Without a list there is nothing to put a delete on, so this is the task that makes cheatsheets manageable at all. The stale comment at `ReportPanel.tsx:182-192` claiming "docs/API.md has no list route to build a list from" must be corrected in the same edit.

- [ ] **Step 1: Add the i18n keys**

`studio.cheatsheet` already has `deleteBody`, `deleteConfirm`, `deleteIt`, `deleting`, `rename`. Add the list's own copy:

`en.json`:
```json
      "listHeading": "Cheatsheets",
      "untitled": "Untitled cheatsheet",
      "renameLabel": "Cheatsheet name"
```
`id.json`:
```json
      "listHeading": "Rangkuman",
      "untitled": "Rangkuman tanpa judul",
      "renameLabel": "Nama rangkuman"
```

- [ ] **Step 2: Add the list to `ReportPanel`**

Below the existing `<CheatsheetForm … />`:

```tsx
      <CheatsheetList workspaceId={workspaceId} />
```

- [ ] **Step 3: Write `CheatsheetList` and `CheatsheetRow`**

Follow `QuizRow`'s structure from Task 10 exactly — the same three-mode state machine, the same in-row confirm block, the same `@md:` title span — with `studio.cheatsheet.*` keys. Status chip labels come from the shared `studio.status.*` keys the quiz row uses, and the status tone map is the same `STATUS_CHIP` record. `CheatsheetStatus` (`'pending' | 'generating' | 'ready' | 'failed'`, `cheatsheets.ts:28`) needs a label for `pending` that the quiz map does not have — use `studio.status.generating` for it, because a `pending` sheet is one whose job has not started, and the student's question is the same.

```tsx
function CheatsheetList({ workspaceId }: { workspaceId: string }) {
  const { t } = useTranslation()
  const sheets = useCheatsheets(workspaceId)
  const list = sheets.data?.cheatsheets

  return (
    <div className="flex flex-col gap-3">
      {list !== undefined && <h2 className="font-medium">{t('studio.cheatsheet.listHeading')}</h2>}
      {sheets.isPending && <SkeletonRows rows={1} label={t('studio.cheatsheet.loading')} />}
      {list !== undefined && list.length === 0 && (
        <EmptyState
          title={t('studio.cheatsheet.empty.title')}
          reason={t('studio.cheatsheet.empty.reason')}
        />
      )}
      {list !== undefined && list.length > 0 && (
        <ul className="flex flex-col divide-y divide-border border-y border-border">
          {list.map((sheet) => (
            <CheatsheetRow key={sheet.id} sheet={sheet} workspaceId={workspaceId} />
          ))}
        </ul>
      )}
    </div>
  )
}
```

`CheatsheetRow` is Task 10's `QuizRow` with: `useDeleteCheatsheet(workspaceId)` / `useRenameCheatsheet(workspaceId)`, `sheet` instead of `quiz`, a `<Link to={`/cheatsheets/${sheet.id}`}>{t('actions.open')}</Link>` as the open action, and `t('studio.cheatsheet.deleteBody')` as the confirm's consequence line (a cheatsheet has no count to interpolate).

Add the `studio.cheatsheet.empty.title` / `.reason` and `studio.cheatsheet.loading` keys to both locales if they are absent — check first with `grep -n '"empty"' app/src/i18n/locales/en.json` scoped to the cheatsheet block.

- [ ] **Step 4: Correct the stale comment**

`ReportPanel.tsx:182-192` says the panel navigates to the new sheet rather than listing every sheet "because … `docs/API.md` has no list route to build a list from." That is no longer true. Rewrite the paragraph to say what the component now does: the form creates and navigates, and the list below it manages what already exists.

- [ ] **Step 5: Verify in the browser**

Run: `npm --prefix app run dev`, open a workspace with a generated cheatsheet. Confirm the sheet appears in the list with Rename and Delete; rename it and confirm the title updates; delete it and confirm it leaves the list.

- [ ] **Step 6: Commit**

```bash
git add app/src/features/studio/ReportPanel.tsx
git commit -m "feat(web): list cheatsheets with delete and rename"
```

---

### Task 13: Deck row — rename

**Files:**
- Modify: `app/src/features/studio/FlashcardsPanel.tsx` (the `DeckRow` function at `:138-249`)
- Modify: `app/src/i18n/locales/en.json`, `app/src/i18n/locales/id.json` (inside `studio.deck`)

**Interfaces:**
- Consumes: `useRenameDeck(workspaceId)` → `mutate({ deckId, title })`.
- Produces: `DeckRow` unchanged in signature.

- [ ] **Step 1: Add the two missing i18n keys**

`studio.deck` already has `deleteConfirm`, `deleteIt`, `deleteNoCards`, `deleteCards_one/_other`, `deleting`, `rename`. Add:

`en.json`:
```json
      "renameLabel": "Deck name"
```
`id.json`:
```json
      "renameLabel": "Nama dek"
```

- [ ] **Step 2: Convert the `confirming` boolean to the mode machine**

`DeckRow` has `const [confirming, setConfirming] = useState(false)`. Replace it with:

```tsx
  const rename = useRenameDeck(workspaceId)
  const [mode, setMode] = useState<'idle' | 'renaming' | 'deleting'>('idle')
  const renameError = asApiError(rename.error)
```

Then:
- Every `confirming` read becomes `mode === 'deleting'`.
- `setConfirming(true)` becomes `setMode('deleting')`; `setConfirming(false)` becomes `setMode('idle')`.
- Add the rename trigger beside the existing delete trigger, inside the same `{mode === 'idle' && ( … )}` guard, using `t('studio.deck.rename')`.
- Add the rename form — identical in shape to Task 10's quiz form, with `name="deck-title"`, `defaultValue={deck.title}`, `t('studio.deck.renameLabel')`, `t('actions.save')`, `t('actions.cancel')`, and `rename.mutate({ deckId: deck.id, title: title.value }, …)`.
- Add the `renameError` notice with `noun={t('errors.nouns.deck')}`.

- [ ] **Step 3: Run the frontend tests**

Run: `npm --prefix app test -- FlashcardsPanel`
Expected: PASS.

- [ ] **Step 4: Commit**

```bash
git add app/src/features/studio/FlashcardsPanel.tsx
git commit -m "feat(web): rename a deck from its row"
```

---

### Task 14: Detail-screen delete

**Files:**
- Modify: `app/src/features/quiz/QuizScreen.tsx`
- Modify: `app/src/features/reports/ReportScreen.tsx`
- Modify: `app/src/features/reports/CheatsheetScreen.tsx`

**Interfaces:**
- Consumes: `useDeleteQuiz(workspaceId)`, `useDeleteReport(workspaceId)`, `useDeleteCheatsheet(workspaceId)`.
- Produces: nothing importable.

No new i18n keys: each screen reuses its type's existing `studio.<type>.deleteConfirm` / `deleteIt` / `deleting`, plus `actions.delete` / `actions.cancel`.

- [ ] **Step 1: Add delete to `QuizScreen`**

`StudyScreen.tsx:684-734` is the template: a `confirming` boolean, a `btn btn-quiet text-danger text-sm self-center` trigger, an inline `note note-danger` confirm block, and `remove.mutate(id, { onSuccess: () => { void navigate(…) } })`.

In `QuizScreen` the workspace id comes from the loaded quiz, and the hook is called with it exactly as `StudyScreen` does with `deck.data?.deck.workspace_id`:

```tsx
  const workspaceId = quiz.data?.quiz.workspace_id ?? ''
  const remove = useDeleteQuiz(workspaceId)
  const [confirmingDelete, setConfirmingDelete] = useState(false)
  const removeError = asApiError(remove.error)
```

On success: `void navigate(`/workspaces/${workspaceId}`)`. Guard the trigger on `workspaceId.length > 0`, as `StudyScreen` does.

- [ ] **Step 2: Add delete to `ReportScreen`**

Same shape with `useDeleteReport(workspaceId)` and `void navigate(`/workspaces/${workspaceId}`)`. Place it in the screen header, beside the title.

- [ ] **Step 3: Add delete to `CheatsheetScreen`**

Same shape with `useDeleteCheatsheet(workspaceId)`, placed in `CheatsheetActions` (`CheatsheetScreen.tsx:254`) alongside re-render and export, and `void navigate(`/workspaces/${workspaceId}`)` on success.

- [ ] **Step 4: Verify each screen**

Run: `npm --prefix app run dev`. For each of the three screens: open it, press Delete, confirm, and check the app lands on the workspace and the item is gone from its list.

- [ ] **Step 5: Commit**

```bash
git add app/src/features/quiz/QuizScreen.tsx app/src/features/reports/ReportScreen.tsx app/src/features/reports/CheatsheetScreen.tsx
git commit -m "feat(web): delete a quiz, report or cheatsheet from its own screen"
```

---

### Task 15: Bulk select and delete

**Files:**
- Modify: `app/src/features/studio/QuizPanel.tsx`, `app/src/features/studio/ReportPanel.tsx`, `app/src/features/studio/FlashcardsPanel.tsx`
- Modify: `app/src/i18n/locales/en.json`, `app/src/i18n/locales/id.json`

**Interfaces:**
- Consumes: `useBulkDelete(workspaceId)` → `mutate(ContentIds)`.
- Produces: nothing importable.

The `studio.bulk` object already exists in both locales with `select`, `deselectAll`, `selected_one/_other`, `deleteConfirm_one/_other`, `deleteBody`, `deleteIt`, `deleting`. Add only `cancel`:

`en.json`: `"cancel": "Cancel selection"` — `id.json`: `"cancel": "Batal pilih"`.

- [ ] **Step 1: Add the selection state to each panel**

```tsx
  const [selecting, setSelecting] = useState(false)
  const [selected, setSelected] = useState<Set<string>>(new Set())
  const bulk = useBulkDelete(workspaceId)
  const bulkError = asApiError(bulk.error)
```

A header button toggles `selecting` and clears `selected` when leaving. While `selecting`, each row renders a checkbox before its title and hides its own rename/delete triggers — a row cannot be in two modes at once. The row components therefore take two new props, `selecting: boolean` and `onSelect: (id: string) => void`, passed from the panel.

- [ ] **Step 2: Add the toolbar**

```tsx
      {selecting && selected.size > 0 && (
        <div className="note flex flex-col items-start gap-2">
          <p className="font-medium">{t('studio.bulk.selected', { count: selected.size })}</p>
          <div className="flex flex-wrap gap-2">
            <button
              type="button"
              className="btn btn-danger text-sm"
              disabled={bulk.isPending}
              onClick={() => {
                bulk.mutate(
                  { quizIds: [...selected] },
                  {
                    onSuccess: () => {
                      setSelected(new Set())
                      setSelecting(false)
                    },
                  },
                )
              }}
            >
              {bulk.isPending ? t('studio.bulk.deleting') : t('studio.bulk.deleteIt')}
            </button>
            <button
              type="button"
              className="btn btn-quiet text-sm"
              onClick={() => { setSelected(new Set()) }}
            >
              {t('studio.bulk.deselectAll')}
            </button>
          </div>
          {bulkError !== null && (
            <ApiErrorNotice error={bulkError} noun={t('errors.nouns.quizzes')} />
          )}
        </div>
      )}
```

Each panel passes only its own id array — `deckIds` for `FlashcardsPanel`, `quizIds` for `QuizPanel`, `reportIds` for `ReportPanel`. `ReportPanel` also owns the cheatsheet list from Task 12, so it keeps a second selection set and sends `cheatsheetIds` in the same call.

The confirm step is the toolbar itself: it names the count and `studio.bulk.deleteBody` states the irreversibility, so the delete button is already the second click. If a reviewer wants a third step, add `t('studio.bulk.deleteConfirm', { count })` as the button's own label instead of the generic `deleteIt` — do not add a modal.

- [ ] **Step 3: Verify**

Run: `npm --prefix app run dev`. In a workspace with several quizzes, enter select mode, select three, delete them, and confirm all three leave the list and the selection clears.

- [ ] **Step 4: Commit**

```bash
git add app/src/features/studio/QuizPanel.tsx app/src/features/studio/ReportPanel.tsx app/src/features/studio/FlashcardsPanel.tsx
git commit -m "feat(web): bulk select and delete across the studio panels"
```

---

### Task 16: Reorder controls

**Files:**
- Modify: `app/src/features/studio/QuizPanel.tsx`, `app/src/features/studio/ReportPanel.tsx`, `app/src/features/studio/FlashcardsPanel.tsx`
- Modify: `app/src/i18n/locales/en.json`, `app/src/i18n/locales/id.json`

**Interfaces:**
- Consumes: `useReorderContent(workspaceId)` → `mutate(ContentIds)`.
- Produces: nothing importable.

- [ ] **Step 1: Add the i18n keys**

`reports.outline.moveUp` / `moveDown` exist but belong to the outline editor, not to a list row. Add a shared pair under `actions`:

`en.json`: `"moveUp": "Move up"`, `"moveDown": "Move down"`
`id.json`: `"moveUp": "Naikkan"`, `"moveDown": "Turunkan"`

- [ ] **Step 2: Add the arrow buttons**

`OutlineEditor` (`ReportScreen.tsx:470-490`) is the precedent: two text-labelled buttons, `disabled` at the ends of the list. There is no drag library in this app and this feature does not add one. There is also no `chevron-up` icon, so the buttons are labelled text, not icons.

Each row takes two props, `onMoveUp?: () => void` and `onMoveDown?: () => void`, and renders both buttons unconditionally, disabling the one whose handler is absent:

```tsx
        <button
          type="button"
          className="btn btn-quiet text-sm"
          disabled={onMoveUp === undefined}
          onClick={onMoveUp}
        >
          {t('actions.moveUp')}
        </button>
        <button
          type="button"
          className="btn btn-quiet text-sm"
          disabled={onMoveDown === undefined}
          onClick={onMoveDown}
        >
          {t('actions.moveDown')}
        </button>
```

The panel passes `undefined` for the first row's up handler and the last row's down handler, so the buttons disable at the ends of the list — the same thing `OutlineEditor` does with `disabled={index === 0}`. Two always-rendered buttons beat conditionally-rendered ones here: the row's layout does not shift as an item moves between the ends and the middle.

- [ ] **Step 3: Send the whole array, not a delta**

The panel computes the reordered array and sends it, because the list order is the server's and `sort_order` is assigned from each array's index:

```tsx
  const order = useReorderContent(workspaceId)
  const move = (ids: string[], from: number, to: number) => {
    const next = [...ids]
    const [moved] = next.splice(from, 1)
    next.splice(to, 0, moved)
    order.mutate({ quizIds: next })
  }
```

Each panel passes its own key — `deckIds`, `quizIds`, `reportIds`.

- [ ] **Step 4: Verify persistence**

Run: `npm --prefix app run dev`. Reorder a list, reload the page, and confirm the order survived the reload — a local-only reorder would look correct until the refetch.

- [ ] **Step 5: Verify the i18n suite goes green**

This is the commit that lands the locale files, so this is where the unused-copy test can finally pass.

Run: `npm --prefix app test -- i18n`
Expected: **10 passed**, with no `feature copy nothing renders` failure. If it still fails, the listed keys are the ones whose component is not rendering them yet — wire those up before committing, or the commit ships a red suite.

- [ ] **Step 6: Commit — the UI and the locale files together**

```bash
git add app/src/features/studio/QuizPanel.tsx app/src/features/studio/ReportPanel.tsx app/src/features/studio/FlashcardsPanel.tsx app/src/i18n/locales/en.json app/src/i18n/locales/id.json
git commit -m "feat(web): reorder decks, quizzes and reports, with the copy the new UI renders"
```

---

### Task 17: Full verification

**Files:** none modified unless a check fails.

- [ ] **Step 1: Backend**

Run: `cargo test --workspace`
Expected: PASS, including `every_registered_route_is_classified` and the tamper suite.

- [ ] **Step 2: Frontend**

Run: `npm --prefix app test && npm --prefix app run build`
Expected: PASS and a clean TypeScript build.

- [ ] **Step 3: Lint**

Run: `just check`
Expected: PASS — clippy with `-D warnings`, `cargo fmt --check`, vitest, the web build, and the CDN check.

- [ ] **Step 4: End-to-end smoke**

Run: `npm --prefix app run dev` with the server up. In one workspace with a deck, a quiz, a report, and a cheatsheet: rename each from its row; delete each from its detail screen; bulk-delete two of one type; reorder a list and reload. Confirm nothing reappears, no stale row lingers, and each list agrees with its detail screen.

- [ ] **Step 5: Commit any fixes**

```bash
git add -A
git commit -m "fix: address verification findings in the content CRUD feature"
```

---

## Notes for the implementer

- **The backend is already written.** Tasks 1–7 add the tests that prove it; if a test fails, the failure is a real defect in the handler, not a missing implementation. Fix the handler.
- **`quiz_attempts` has no `deleted_at`.** Do not add one. The dashboard counts work already done, and a delete that erased attempts would rewrite a figure the student has already seen.
- **Reports have no child table to tombstone.** `report_revisions` is a server-side recovery copy with no `deleted_at`; it cascades on a real row delete, which only the account purge performs.
- **Cheatsheets keep their object keys.** `blocks_key`, `html_key`, `pdf_key` point into the bucket; the account purge removes the bytes. Nulling them on delete would orphan the objects, because nothing else records their names.
- **`sort_order` defaults to 0 for every existing row.** A list that was never reordered therefore sorts by `created_at desc, id` exactly as before, which is why the migration needs no backfill.
- **`useBulkDelete` and `useReorderContent` invalidate `['workspaces', id]`**, which prefix-matches every content list for that workspace. A new list key added later must live under `['workspaces', id, …]` or these two will silently miss it.
