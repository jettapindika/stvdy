//! `purge_deleted_accounts`: the hard delete behind `DELETE /account` (A355).
//!
//! The job kind has been named in `migrations/0015_jobs.sql` since Phase 2 and this is the
//! handler that dispatches it. It runs at the `run_at` the deletion request scheduled —
//! `deleted_at + ACCOUNT_DELETION_GRACE_DAYS` — so the window lives in exactly one place and
//! the handler purges whatever it is given.
//!
//! # What is deleted, what is kept, and why
//!
//! The spec's rule is *"deletes or anonymizes all user data"*, and the two halves are not
//! interchangeable. A purge that deletes the payment ledger breaks the margin report and the
//! tax position; a purge that keeps the messages has deleted nothing. So the mapping is
//! written out here, one group at a time, rather than expressed as "delete everything that
//! cascades from `users`" — which is precisely the mistake, because `payments`,
//! `credit_ledger`, `subscriptions` and `usage_events` all carry `on delete cascade` and
//! deleting the user row would take them with it.
//!
//! **Deleted — the student's content and account state.** Every table in [`CONTENT_TABLES`]
//! holds something the student wrote, uploaded or generated, or a piece of the account's own
//! configuration. `chunks` (extracted text), `reminder_deliveries` and `push_tokens` have no
//! `user_id` and go with their parents by foreign key; the comment on the list names which
//! cascade covers which.
//!
//! **Retained, attached to an anonymized account — financial records and audit.** `payments`
//! (what was charged), `credit_ledger` (the balance's history), `subscriptions` (the periods
//! paid for), `usage_events` (the provider cost the margin report is built from) and
//! `discount_redemptions` (which code was spent). `docs/EULA.md` § 9 says records required for
//! tax or accounting survive deletion "in anonymised or legally-required form", and this is
//! that form.
//!
//! **Retained — audit with no personal content.** `change_log` and `mutation_log` hold entity
//! names, ids and operation types; `gateway_events` holds the gateway's own answers and has
//! no `user_id` column at all. None of them can be read back into a student's data, and the
//! `change_log` rows a purge writes are the tombstones of the rows it just removed.
//!
//! **The account row is anonymized, not deleted.** That is what makes the retained half
//! possible: `payments.user_id` and `credit_ledger.user_id` are `not null references users on
//! delete cascade`, so removing the row would remove the ledger. The row becomes a tombstone —
//! no address, no password, no Google subject, no feed token, no admin flag, the free plan —
//! which is exactly "anonymize" in the sense the spec uses, and it releases the student's real
//! email address for a new account.
//!
//! # Idempotence
//!
//! Every step is a fixed point of itself: the deletes match nothing the second time, the
//! retained rows are set to values they already hold, and `purged_at` is written with
//! `coalesce` so a second pass cannot move the date. A second run is therefore a no-op that
//! reports zero rows rather than an error — which is what makes it safe to call from a test, a
//! retry, or a scheduler pass that raced the request's own enqueue.

use uuid::Uuid;

use crate::jobs::{ClaimedJob, Failure};
use crate::state::AppState;

/// The tables whose rows are the student's, deleted by `user_id`.
///
/// Written as one list of statements rather than a table-name loop for the reason the export's
/// collection list is: the predicate is the safety property, and a list you can read is a list
/// you can check. Order matters only where a foreign key would otherwise be violated first —
/// children before the workspaces they hang off — and the cascades named in the comment are
/// what covers the three tables that have no `user_id` of their own.
const CONTENT_TABLES: &[&str] = &[
    // Chat, then the workspace tree it hangs off. `chunks` (via `sources`) and
    // `syllabus_drafts` (via `sources`) cascade.
    "messages",
    "sources",
    "workspaces",
    // Study material. `quiz_questions` and `quiz_attempts` cascade from `quizzes`, and
    // `report_revisions` from `reports`; they are listed anyway so a future migration that
    // drops one of those cascades cannot silently leave rows behind.
    "flashcards",
    "flashcard_reviews",
    "decks",
    "quiz_questions",
    "quiz_attempts",
    "quizzes",
    "report_revisions",
    "reports",
    "cheatsheets",
    // Planning and study state.
    "tasks",
    "goals",
    "study_sessions",
    "events",
    "plan_proposals",
    "syllabus_drafts",
    "pending_actions",
    // Reminders, then their deliveries by cascade.
    "reminders",
    // The calendar connection carries an encrypted Google refresh token: deleting it is part
    // of deleting the account, and it is the row that would otherwise let a purge leave a
    // live credential pointed at the student's own calendar.
    "calendar_connections",
    // Account state and credentials. `push_tokens` cascade from `devices`.
    "user_settings",
    "user_api_keys",
    "auth_sessions",
    "devices",
    "email_verification_tokens",
    "login_tickets",
    // Sync machinery with no meaning for an unreachable account. `mutation_log.result` is a
    // stored response body and can contain the student's own content, which is why it goes
    // rather than being retained like `change_log`.
    "mutation_log",
    "change_log_watermark",
];

/// Runs the job. The payload carries the account; the window was the job's `run_at`.
pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let user_id = job.user_id()?;
    let outcome = purge(state, user_id).await?;

    tracing::info!(
        %user_id,
        rows_deleted = outcome.rows_deleted,
        anonymized = outcome.anonymized,
        already_purged = outcome.already_purged,
        "purged a deleted account"
    );
    Ok(())
}

/// What one purge pass did.
#[derive(Debug)]
pub struct Purge {
    /// Rows removed from the content tables. Zero on a second pass, which is the idempotence.
    pub rows_deleted: u64,
    /// Object-storage keys handed to the storage backend. Reported so a test can assert the
    /// bytes were reached, not only the rows.
    pub objects_deleted: u64,
    /// True when this pass anonymized the account row (false when it already was).
    pub anonymized: bool,
    /// True when the account had already been purged, so this pass only confirmed it.
    pub already_purged: bool,
}

/// The object-storage keys the account's rows point at, read **before** the rows go.
///
/// `sources.file_key` is the uploaded original (and, for a converted kind, the preview's key is
/// derived from it by the same row); `cheatsheets` carries the rendered HTML and PDF. Both are
/// bytes in object storage that nothing removes when the row disappears, which is why the purge
/// has to name them: without this, a deleted account's PDFs and rendered documents sit in the
/// bucket forever, and `docs/OPERATIONS.md` § Retention already promises account deletion as the
/// mechanism that removes them.
///
/// Read in the same transaction as the deletes, and the storage calls happen after the commit —
/// the ordering `workspaces::remove` and `sources::routes::remove` both use. A storage delete
/// inside a transaction that then rolled back would leave a live row pointing at bytes that are
/// gone, which is worse than an orphan.
const STORAGE_KEYS: &[(&str, &str)] = &[
    (
        "sources",
        "select file_key from sources where user_id = $1 and file_key is not null",
    ),
    (
        "cheatsheets",
        "select blocks_key from cheatsheets where user_id = $1 and blocks_key is not null",
    ),
    (
        "cheatsheets",
        "select html_key from cheatsheets where user_id = $1 and html_key is not null",
    ),
    (
        "cheatsheets",
        "select pdf_key from cheatsheets where user_id = $1 and pdf_key is not null",
    ),
];

/// The keys the purge will remove from storage, read before the rows are deleted.
async fn storage_keys(
    conn: &mut sqlx::PgConnection,
    user_id: Uuid,
) -> Result<Vec<String>, Failure> {
    let mut keys = Vec::new();
    for (table, statement) in STORAGE_KEYS {
        let found: Vec<Option<String>> = sqlx::query_scalar(*statement)
            .bind(user_id)
            .fetch_all(&mut *conn)
            .await
            .map_err(|error| Failure::transient(format!("reading {table}'s objects: {error}")))?;
        keys.extend(found.into_iter().flatten().filter(|key| !key.is_empty()));
    }
    Ok(keys)
}

/// Removes a marked account's data and anonymizes its row. Idempotent.
///
/// One transaction: a purge that deleted the messages and then failed to anonymize the row
/// would leave an account whose address is still on file after the student was told it was
/// gone, which is the one outcome this job exists to prevent.
pub async fn purge(state: &AppState, user_id: Uuid) -> Result<Purge, Failure> {
    let now = state.clock.now();

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| Failure::transient(format!("beginning the purge: {error}")))?;

    // The account, locked for the pass. `for update` so a second worker — or a request that
    // arrived at the same moment — waits rather than racing the same deletes.
    let marked: Option<Option<chrono::DateTime<chrono::Utc>>> =
        sqlx::query_scalar("select deleted_at from users where id = $1 for update")
            .bind(user_id)
            .fetch_optional(&mut *tx)
            .await
            .map_err(|error| Failure::transient(format!("loading the account: {error}")))?;

    match marked {
        // A payload naming an account that does not exist is nothing to do, and retrying
        // cannot change that.
        None => {
            tracing::warn!(%user_id, "purge job for an account that does not exist");
            return Ok(Purge {
                rows_deleted: 0,
                objects_deleted: 0,
                anonymized: false,
                already_purged: true,
            });
        }
        // A live account. The only producer is the deletion request and the scheduler's
        // backstop, both of which filter on `deleted_at is not null`, so this is a row that
        // should not exist: a permanent failure rather than a retry, because no retry will
        // mark an account the student did not ask to delete.
        Some(None) => {
            return Err(Failure::permanent(
                "account is not marked for deletion; refusing to purge it",
            ));
        }
        Some(Some(_)) => {}
    }

    let already: bool =
        sqlx::query_scalar::<_, bool>("select purged_at is not null from users where id = $1")
            .bind(user_id)
            .fetch_one(&mut *tx)
            .await
            .map_err(|error| Failure::transient(format!("reading the purge date: {error}")))?;

    let mut rows_deleted = 0u64;
    // Read the object keys before the rows that name them go: after the deletes there is
    // nothing left to tell us which bytes belonged to this account.
    let keys = storage_keys(&mut tx, user_id).await?;

    for table in CONTENT_TABLES {
        // `AssertSqlSafe` because the statement is built from a table name: the name comes from
        // [`CONTENT_TABLES`], a `&'static str` literal list in this file, and never from a
        // request or a row — so the only thing the opt-in suppresses is the compiler's inability
        // to see that. The predicate is still a bind parameter.
        let affected = sqlx::query(sqlx::AssertSqlSafe(format!(
            "delete from {table} where user_id = $1"
        )))
        .bind(user_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| Failure::transient(format!("purging {table}: {error}")))?
        .rows_affected();
        rows_deleted += affected;
    }

    // A paid period that is over is a fact about money, and it is retained; a paid period that
    // still looks live would be a row the rollover and the margin report read as current
    // revenue for an account that no longer exists. Cancelling it is the honest state, and the
    // `payments` rows that produced it are untouched.
    sqlx::query(
        "update subscriptions
            set status = 'canceled', pending_plan_id = null, pending_billing_period = null
          where user_id = $1 and status <> 'canceled'",
    )
    .bind(user_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| Failure::transient(format!("cancelling the subscriptions: {error}")))?;

    // The tombstone. `coalesce(purged_at, $2)` rather than `purged_at = $2`: a second pass must
    // not move the date it recorded the first time, which is the fact `docs/OPERATIONS.md`'s
    // retention table and the scheduler's backstop both read.
    let anonymized = sqlx::query(
        "update users
            set email = 'deleted-' || id::text || '@deleted.invalid',
                password_hash = null,
                google_sub = null,
                ics_feed_token = null,
                student_email = null,
                timezone = 'Asia/Jakarta',
                plan_id = 'free',
                email_verified = false,
                email_verified_at = null,
                student_verified = false,
                is_admin = false,
                purged_at = coalesce(purged_at, $2)
          where id = $1",
    )
    .bind(user_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|error| Failure::transient(format!("anonymizing the account: {error}")))?
    .rows_affected()
        > 0;

    tx.commit()
        .await
        .map_err(|error| Failure::transient(format!("committing the purge: {error}")))?;

    // **After** the commit, and logging rather than failing — the same rule the source and
    // workspace deletes follow. A storage delete that failed must not fail the purge: an account
    // left half-deleted because R2 was briefly unavailable is worse than an orphaned object, and
    // the orphan is the accepted residual. `delete_objects` warns per key, and the R2 lifecycle
    // rule on the `reports/` and `cheatsheets/` prefixes (`docs/OPERATIONS.md` § Retention) is
    // the backstop for one that is never retried.
    crate::sources::routes::delete_objects(state, &keys).await;

    Ok(Purge {
        rows_deleted,
        objects_deleted: keys.len() as u64,
        anonymized,
        already_purged: already,
    })
}
