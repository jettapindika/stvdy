//! Pending actions: the proposals an agent turn leaves behind for a human to confirm.
//!
//! A WRITE tool never executes (A271). It becomes a row here — `tool`, the arguments the
//! model produced, and a status — and the only thing that turns that row into a domain row
//! is [`routes::confirm`], which runs [`execute`] **inside the claim's own transaction**.
//! Three properties follow, and each one is a test:
//!
//!  * **Single winner** (A275). Two devices confirming the same proposal must produce one
//!    row, so the transition is a guarded `update … where status = 'pending' returning …`
//!    rather than a read-then-write: the loser gets zero rows and answers `409` having
//!    written nothing. [`claim`] takes a `&mut PgTransaction` rather than a pool, so the
//!    call site cannot forget to put the claim and the execution in one transaction — the
//!    type is the reminder.
//!  * **No second implementation** (A276). [`execute`] contains no DML of its own: every
//!    arm calls the same service function the REST route calls, so a confirmed task is the
//!    row `POST /tasks` would have written, with the same validation and the same
//!    `change_log` entry.
//!  * **Lazy expiry** (A274). A proposal past `expires_at` reads `expired` on the next list
//!    with no job having run, and the claim's own `expires_at > now()` is the enforcement —
//!    the list is only the convenience.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use crate::error::{AppError, AppResult};

pub mod execute;
pub mod routes;

pub use routes::routes;

/// How long a proposal stays actionable.
///
/// The migration that adds the column deliberately carries **no SQL default** (A273): this
/// constant is the only place the number lives, and every writer sets the column
/// explicitly. `migrations/0033_pending_actions_resolution.sql` backfills rows that predate
/// the column with the same 30 minutes, which is a one-time value rather than a default.
pub const PENDING_ACTION_TTL_MIN: i64 = 30;

/// How long a resolved proposal stays readable in `GET /actions/pending`.
///
/// The card that was confirmed on device A has to show "Confirmed" on device B rather than
/// vanish, and a client that was offline across the resolution has no other way to learn
/// what happened. A day is long enough for that and short enough that the list stays a
/// list of things to decide about.
pub const RESOLVED_VISIBLE_HOURS: i64 = 24;

/// Which way a proposal was resolved.
///
/// Two variants and no third: `expired` is not a resolution a client asks for (A274 makes
/// it the server's own transition), and a `Resolution::Expired` would let a caller claim it.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Resolution {
    Confirmed,
    Rejected,
}

impl Resolution {
    fn as_str(self) -> &'static str {
        match self {
            Resolution::Confirmed => "confirmed",
            Resolution::Rejected => "rejected",
        }
    }
}

/// The row as the server reads it.
///
/// `args` is the object the model produced, stored verbatim: the proposal is what the
/// student sees on the card, and re-serialising it into a typed shape would make the card
/// and the confirm disagree about what was proposed.
///
/// `user_id` is deliberately **not** a field: every statement that reads this type is already
/// scoped by the caller's id, so a copy on the struct would be a second, unchecked answer to
/// "whose row is this" — and the one place it would be read is the one place a scoping bug
/// would hide.
#[derive(Debug, sqlx::FromRow)]
pub(crate) struct ActionRow {
    pub id: Uuid,
    pub tool: String,
    pub args: serde_json::Value,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_device_id: Option<Uuid>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub message_id: Option<Uuid>,
}

/// The row as every client sees it. `status` is the server's, never the client's.
#[derive(Debug, serde::Serialize)]
pub struct ActionView {
    pub id: Uuid,
    pub tool: String,
    pub args: serde_json::Value,
    pub status: String,
    pub created_at: Option<DateTime<Utc>>,
    pub expires_at: DateTime<Utc>,
    pub resolved_at: Option<DateTime<Utc>>,
    pub resolved_by_device_id: Option<Uuid>,
    pub result: Option<serde_json::Value>,
    pub error: Option<String>,
    pub message_id: Option<Uuid>,
}

impl From<ActionRow> for ActionView {
    fn from(row: ActionRow) -> Self {
        Self {
            id: row.id,
            tool: row.tool,
            args: row.args,
            status: row.status,
            created_at: row.created_at,
            expires_at: row.expires_at,
            resolved_at: row.resolved_at,
            resolved_by_device_id: row.resolved_by_device_id,
            result: row.result,
            error: row.error,
            message_id: row.message_id,
        }
    }
}

/// Every column a read or a returning write selects, in the order [`ActionRow`] declares.
///
/// One list rather than three literals, because the claim's `returning` and the list's
/// `select` must agree: a column added to one and not the other is a decode failure at
/// runtime, which is a 500 on a route the schema test would have caught if the list were
/// derived from the struct instead of written twice.
pub(crate) const ACTION_COLUMNS: &str = "id, tool, args, status, created_at, \
     expires_at, resolved_at, resolved_by_device_id, result, error, message_id";

/// The lazy expiry (A274). One statement, and the caller is the caller's own rows.
///
/// A GET that writes is not a smell here: the alternative is a scheduler pass over every
/// account's pending actions, and the roadmap § 8 job-kind list is fixed and has no expiry
/// kind. The claim re-checks `expires_at` inside its own transaction, so this is a
/// convenience for the list rather than the enforcement.
pub(crate) async fn expire_stale(pool: &PgPool, user_id: Uuid) -> AppResult<u64> {
    let affected = sqlx::query(
        "update pending_actions
            set status = 'expired', resolved_at = now()
          where user_id = $1 and status = 'pending' and expires_at <= now()",
    )
    .bind(user_id)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("expiring stale proposals: {error}")))?;
    Ok(affected.rows_affected())
}

/// The single-winner claim (A275). `Ok(None)` means someone else already resolved it.
///
/// The `and status = 'pending' and expires_at > now()` predicates are the whole mechanism:
/// the second tap matches zero rows and answers `409` having written nothing, and an expired
/// proposal cannot be confirmed even when the lazy expiry has not run. `resolved_by_device_id`
/// is what makes the exit gate's "confirm on device B" a fact rather than a claim.
///
/// `result` is written by [`record_result`] once the executor has run, because the claim's
/// own `returning` happens before there is a result to store.
pub(crate) async fn claim(
    tx: &mut sqlx::PgTransaction<'_>,
    user_id: Uuid,
    id: Uuid,
    device_id: Uuid,
    status: Resolution,
) -> AppResult<Option<ActionRow>> {
    let sql = format!(
        "update pending_actions
            set status = $4, resolved_at = now(), resolved_by_device_id = $3
          where id = $1 and user_id = $2 and status = 'pending' and expires_at > now()
         returning {ACTION_COLUMNS}"
    );
    sqlx::query_as::<_, ActionRow>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(user_id)
        .bind(device_id)
        .bind(status.as_str())
        .fetch_optional(&mut **tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("claiming a proposal: {error}")))
}

/// The row, whatever its status, or `None` when it is not the caller's.
///
/// Separate from [`claim`] because the two answer different questions: this one is what
/// makes another account's proposal a `404` rather than a `409` (a `409` would confirm the
/// id exists), and the claim is what makes a second resolution a `409`.
pub(crate) async fn find(pool: &PgPool, user_id: Uuid, id: Uuid) -> AppResult<Option<ActionRow>> {
    let sql = format!(
        "select {ACTION_COLUMNS} from pending_actions
          where id = $1 and user_id = $2 and deleted_at is null"
    );
    sqlx::query_as::<_, ActionRow>(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a proposal: {error}")))
}

/// Records what the execution produced, inside the claim's transaction.
///
/// The `and status = 'confirmed'` predicate is not decoration: it makes a write to a row
/// this call did not claim impossible, so a `result` can never be attached to a proposal
/// that is still pending or that someone else resolved.
pub(crate) async fn record_result(
    tx: &mut sqlx::PgTransaction<'_>,
    user_id: Uuid,
    id: Uuid,
    result: &serde_json::Value,
) -> AppResult<()> {
    sqlx::query(
        "update pending_actions set result = $3
          where id = $1 and user_id = $2 and status = 'confirmed'",
    )
    .bind(id)
    .bind(user_id)
    .bind(result)
    .execute(&mut **tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a result: {error}")))?;
    Ok(())
}

/// Records why a confirmed proposal did not finish, after the claim has committed.
///
/// This is the one failure the rollback cannot cover: the proposal *was* resolved and the
/// row it produced *was* written, and then the work that row needs — a generation job —
/// could not be queued. Rolling the claim back is no longer possible (it is committed), so
/// the honest outcome is a `confirmed` row carrying the reason, which is what the card on
/// every device then shows.
pub(crate) async fn record_error(
    pool: &PgPool,
    user_id: Uuid,
    id: Uuid,
    error: &str,
) -> AppResult<()> {
    sqlx::query(
        "update pending_actions set error = $3
          where id = $1 and user_id = $2 and status = 'confirmed'",
    )
    .bind(id)
    .bind(user_id)
    .bind(error)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a failure: {error}")))?;
    Ok(())
}
