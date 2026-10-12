//! One pass over one connection.
//!
//! Two halves, and they fail differently enough to be separate functions:
//!
//!  * [`sync_ics`] is a **conditional GET plus a full re-upsert**. Its two non-obvious rules
//!    are A239's — a `304` advances `last_synced_at` and touches nothing else, because
//!    `events` is `+sync` and an unconditional re-upsert writes one `change_log` row per
//!    event per poll, which every device then pulls — and A232/A247's — a truncated expansion
//!    keeps every occurrence it read and records `recurrence_truncated` from
//!    [`Expansion::truncated`], **never** from `occurrences.len()`.
//!  * The Google half is Task 7's; it shares [`SyncOutcome`] and the upsert key.
//!
//! **What writes what.** This module writes the four columns A198 reserves —
//! `source`, `external_id`, `etag`, `connection_id` — through plain SQL, which is exactly why
//! the read-only rule lives in the routes and the push validator rather than in a trigger
//! (A240): a trigger that refused the write would refuse the sync's own write, and the only
//! way to keep the two apart would be a session variable.
//!
//! **Why the writes are idempotent.** The key is `(connection_id, external_id)` (A231), so a
//! re-poll updates the same rows rather than inserting a second copy of every class. That is
//! what makes a poll safe to repeat after a crash, and what makes the 304 worth having.

use axum::Json;
use axum::extract::State;
use axum::http::StatusCode;
use chrono::{DateTime, Utc};
use serde_json::{Value, json};
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use stvdy_core::calendar::expand::{HORIZON_DAYS, Occurrence, Window, expand_feed};
use stvdy_core::entitlements::Feature;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::integrations::fetch::Validators;
use crate::state::AppState;

/// The `error` value a truncated expansion records (A232).
///
/// A constant because three places read it: this module writes it, the API documents it, and
/// a test asserts it. Three string literals would drift.
pub const RECURRENCE_TRUNCATED: &str = "recurrence_truncated";

/// What one pass did. `was_full` is A236's mode flag: for a Google connection it says whether
/// the pass sent a `sync_token`, and for an ICS connection every pass that fetched a body is
/// a full listing.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct SyncOutcome {
    pub inserted: u32,
    pub updated: u32,
    pub deleted: u32,
    pub was_full: bool,
}

/// One `calendar_connections` row, as the sync needs it.
///
/// An explicit struct rather than a `Value`: the sync reads fields it must not get wrong
/// (the validators, the URL), and a JSON bag would make a typo a silent `None`.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ConnectionRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub provider: String,
    pub account_email: Option<String>,
    pub sync_token: Option<String>,
    pub ics_url: Option<String>,
    pub last_synced_at: Option<DateTime<Utc>>,
    pub status: String,
    pub error: Option<String>,
    pub ics_etag: Option<String>,
    pub ics_last_modified: Option<String>,
    pub study_calendar_id: Option<String>,
    pub last_sync_was_full: bool,
}

/// The columns every read of a connection selects (A27: an explicit list, never `select *`).
const CONNECTION_COLUMNS: &str = "id, user_id, provider, account_email, sync_token, ics_url, \
     last_synced_at, status, error, ics_etag, ics_last_modified, study_calendar_id, \
     last_sync_was_full";

/// One connection by id.
pub async fn load(pool: &PgPool, id: Uuid) -> AppResult<Option<ConnectionRow>> {
    let sql = format!("select {CONNECTION_COLUMNS} from calendar_connections where id = $1");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a connection: {error}")))
}

/// The caller's connections, oldest first.
pub async fn load_for_user(pool: &PgPool, user_id: Uuid) -> AppResult<Vec<ConnectionRow>> {
    let sql = format!(
        "select {CONNECTION_COLUMNS} from calendar_connections
          where user_id = $1 order by created_at asc, id asc"
    );
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_all(pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("listing connections: {error}")))
}

/// A connection as a client sees it (A249).
///
/// **No token of any kind**: no `refresh_token_enc`, no `sync_token`, and no `ics_url` either
/// — a per-user feed URL can carry a credential in its query string, and A249 fixes the field
/// list. `account_email` is the one Google fact worth storing, because it is what lets a
/// student tell two accounts apart.
pub fn view(row: &ConnectionRow) -> Value {
    json!({
        "id": row.id,
        "provider": row.provider,
        "account_email": row.account_email,
        "status": row.status,
        "error": row.error,
        "last_synced_at": row.last_synced_at,
        "study_calendar_id": row.study_calendar_id,
    })
}

/// One pass over one ICS connection: conditional GET, parse, bounded expand, upsert on
/// `(connection_id, external_id)`. A `304` advances `last_synced_at` only (A239).
///
/// The window starts at the **user's local day start**, not at `now`: a first poll at 10:00
/// would otherwise drop this morning's 08:00 class, and "the class I am sitting in is missing
/// from the app" is the kind of gap a student cannot diagnose.
pub async fn sync_ics(state: &AppState, connection: &ConnectionRow) -> AppResult<SyncOutcome> {
    let Some(url) = connection.ics_url.as_deref().filter(|url| !url.is_empty()) else {
        // A row with no URL cannot be fixed by retrying.
        record_failure(state, connection, "this ICS connection has no URL").await?;
        return Ok(SyncOutcome::default());
    };

    let validators = Validators {
        etag: connection.ics_etag.clone(),
        last_modified: connection.ics_last_modified.clone(),
    };
    let conditional = validators.etag.is_some() || validators.last_modified.is_some();

    let started = std::time::Instant::now();
    let fetched = match state
        .ics_fetch
        .get(url, conditional.then_some(&validators))
        .await
    {
        Ok(fetched) => fetched,
        Err(error) => {
            // The validators are **dropped** with the error, deliberately: after a failed
            // fetch we know nothing about the remote document, so the next pass must be
            // unconditional. Keeping them would let a server answer `304` to a request whose
            // body we never read, and the connection would hold an error that no pass could
            // clear.
            record_failure(state, connection, &error.to_string()).await?;
            tracing::warn!(
                connection_id = %connection.id,
                provider = "ics",
                latency_ms = started.elapsed().as_millis() as u64,
                %error,
                "the ICS fetch failed"
            );
            return Ok(SyncOutcome::default());
        }
    };

    // A239: an unchanged feed touches **nothing**. Not the events — an unconditional re-upsert
    // of a `+sync` table writes one `change_log` row per event per poll, and every device then
    // pulls the whole calendar every fifteen minutes — and not the validators, which are
    // already correct.
    if fetched.not_modified {
        sqlx::query(
            "update calendar_connections
                set last_synced_at = now(), updated_at = now()
              where id = $1",
        )
        .bind(connection.id)
        .execute(&state.pool)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("recording a conditional GET: {error}"))
        })?;
        tracing::info!(
            connection_id = %connection.id,
            provider = "ics",
            latency_ms = started.elapsed().as_millis() as u64,
            "the ICS feed is unchanged"
        );
        return Ok(SyncOutcome {
            was_full: true,
            ..SyncOutcome::default()
        });
    }

    let from = local_day_start(state, connection.user_id).await?;
    let expansion = match expand_feed(&fetched.body, Window::days_from(from, HORIZON_DAYS)) {
        Ok(expansion) => expansion,
        Err(error) => {
            record_failure(state, connection, &error.to_string()).await?;
            tracing::warn!(
                connection_id = %connection.id,
                provider = "ics",
                %error,
                "the ICS body could not be expanded"
            );
            return Ok(SyncOutcome::default());
        }
    };

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning an ICS import: {error}"))
        })?;
    let mut outcome = SyncOutcome {
        was_full: true,
        ..SyncOutcome::default()
    };
    for occurrence in &expansion.occurrences {
        if upsert_occurrence(&mut tx, connection, occurrence).await? {
            outcome.inserted += 1;
        } else {
            outcome.updated += 1;
        }
    }

    // A247: a partial import keeps what it read and records the reason, and the job succeeds.
    // One malformed recurring rule in a faculty calendar must not mean the student gets
    // nothing, and a job that *failed* would retry the same broken feed five times and record
    // the same error.
    let error = expansion_error(&expansion);
    let status = if error.is_some() {
        "error"
    } else {
        "connected"
    };
    sqlx::query(
        "update calendar_connections
            set status = $2,
                error = $3,
                ics_etag = $4,
                ics_last_modified = $5,
                last_synced_at = now(),
                updated_at = now()
          where id = $1",
    )
    .bind(connection.id)
    .bind(status)
    .bind(error.as_deref())
    .bind(fetched.etag.as_deref())
    .bind(fetched.last_modified.as_deref())
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording an ICS import: {error}")))?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing an ICS import: {error}"))
    })?;

    tracing::info!(
        connection_id = %connection.id,
        provider = "ics",
        inserted = outcome.inserted,
        updated = outcome.updated,
        skipped = expansion.skipped.len(),
        truncated = expansion.truncated,
        latency_ms = started.elapsed().as_millis() as u64,
        "the ICS feed was imported"
    );
    Ok(outcome)
}

/// The reason a pass that read a body still has an error, or `None` for a clean pass.
///
/// **Reads `truncated`, never `occurrences.len()`** (A232). The two agree on every feed with
/// fewer than [`stvdy_core::calendar::expand::MAX_OCCURRENCES`] occurrences and disagree
/// exactly where it matters, and the disagreement is one-sided: the length says "truncated"
/// only when the cap fired, so a connection that legitimately has 500 occurrences is fine —
/// while `truncated` is also true when the engine gave up mid-iteration, which the length
/// cannot see at all.
fn expansion_error(expansion: &stvdy_core::calendar::expand::Expansion) -> Option<String> {
    let skipped = (!expansion.skipped.is_empty()).then(|| {
        format!(
            "{} event(s) could not be read: {}",
            expansion.skipped.len(),
            expansion.skipped.join("; ")
        )
    });
    match (expansion.truncated, skipped) {
        (true, Some(skipped)) => Some(format!("{RECURRENCE_TRUNCATED}; {skipped}")),
        (true, None) => Some(RECURRENCE_TRUNCATED.to_string()),
        (false, skipped) => skipped,
    }
}

/// The upsert. `on conflict (connection_id, external_id) where external_id is not null do
/// update` — the partial index's predicate must be repeated, exactly as the roadmap's
/// `dedupe_key` note records for the same reason: without it Postgres cannot infer which
/// index the conflict is against and the statement is a syntax error.
///
/// `returns true` for an insert and `false` for an update. `xmax = 0` is what tells the two
/// apart: a row this statement created has no locking transaction id, and the `do update`
/// path sets one on the new tuple. The alternative — a `select` before every write — is a
/// second round trip per occurrence, on a table whose whole point is that a poll is cheap.
async fn upsert_occurrence(
    tx: &mut PgConnection,
    connection: &ConnectionRow,
    occurrence: &Occurrence,
) -> AppResult<bool> {
    let inserted: bool = sqlx::query_scalar(
        "insert into events
            (id, user_id, kind, title, starts_at, ends_at, external_id, connection_id, source)
         values ($1, $2, 'external', $3, $4, $5, $6, $7, 'ics')
         on conflict (connection_id, external_id) where external_id is not null
         do update set title = excluded.title,
                       starts_at = excluded.starts_at,
                       ends_at = excluded.ends_at,
                       deleted_at = null
         returning xmax = 0",
    )
    .bind(Uuid::now_v7())
    .bind(connection.user_id)
    .bind(&occurrence.title)
    .bind(occurrence.starts_at)
    .bind(occurrence.ends_at)
    .bind(occurrence_key(occurrence))
    .bind(connection.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("importing an occurrence: {error}")))?;

    Ok(inserted)
}

/// `events.external_id` for an ICS occurrence (A231): `"<UID>|<recurrence-id in UTC>"` for a
/// recurring series, and `"<UID>"` alone for a one-off.
///
/// The occurrence, not the series, is the row: `events` has no `rrule` column, and Phase 11's
/// planner reads these rows as busy time. Storing the series and expanding on read would need
/// a second recurrence engine in the planner, in the SPA's grid and in the feed, all three of
/// which would drift.
///
/// The instant is formatted at second granularity in UTC. `to_rfc3339()` would be equivalent
/// but carries an offset spelling that varies with the value's zone; a fixed format means the
/// key is a function of the instant alone, which is what makes a re-poll match.
fn occurrence_key(occurrence: &Occurrence) -> String {
    match occurrence.recurrence_id {
        Some(recurrence_id) => format!(
            "{}|{}",
            occurrence.uid,
            recurrence_id.format("%Y-%m-%dT%H:%M:%SZ")
        ),
        None => occurrence.uid.clone(),
    }
}

/// `POST /calendar/sync`. Enqueues the same kind the poll uses, with a **per-minute** dedupe
/// key, and answers `202 { "queued": n }` (A238).
///
/// Why the same row rather than an inline sync: a Google pass is several round trips to
/// Google, and holding a request open across them makes a timeout the student's problem. The
/// work is the worker's, and the request's job is to make a row.
///
/// Why a per-minute key rather than the poll's window: a student who has just added a class
/// and wants it now must not be told to wait fifteen minutes because the poll already ran.
/// The per-minute window bounds a retry loop without a second rate limiter, which is the
/// reason the plan gives for the shape.
///
/// `queued` is the number of jobs actually *inserted*, so a second press inside the minute
/// answers `{"queued": 0}` rather than a lie about work that is not going to happen. That is
/// also what makes the answer assertable.
pub async fn manual_sync(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<(StatusCode, Json<Value>)> {
    crate::entitlements::require(&state, &user, Feature::GoogleCalendar).await?;

    let connections = load_for_user(&state.pool, user.user_id).await?;
    let window = crate::jobs::schedule::manual_window(&state);
    let mut queued = 0u32;

    for connection in &connections {
        // A revoked Google connection is not worth a job: the pass would mark it revoked
        // again, and the student's remedy is to reconnect rather than to press this.
        if connection.status == "revoked" {
            continue;
        }
        let dedupe_key = format!("sync_calendar:{}:manual:{window}", connection.id);
        let inserted: Option<Uuid> = sqlx::query_scalar(
            "insert into jobs (kind, payload, dedupe_key)
             values ('sync_calendar', $1, $2)
             on conflict (dedupe_key) where dedupe_key is not null do nothing
             returning id",
        )
        .bind(json!({
            "user_id": user.user_id,
            "connection_id": connection.id,
        }))
        .bind(&dedupe_key)
        .fetch_optional(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("enqueuing a manual sync: {error}")))?;
        if inserted.is_some() {
            queued += 1;
        }
    }

    if queued > 0 {
        state.jobs.wake();
    }
    Ok((StatusCode::ACCEPTED, Json(json!({ "queued": queued }))))
}

/// Records a failure on the connection, clearing the validators so the next pass is
/// unconditional, and advancing `last_synced_at` so the poll does not re-enqueue every tick.
///
/// Advancing the watermark on a *failure* is deliberate: the poll's query is "older than the
/// interval", so not advancing means a dead URL is retried on every scheduler pass — a retry
/// storm from a row nobody is watching. The screen shows `status = 'error'` and the reason
/// beside it, which is what keeps the timestamp honest (gate 7).
async fn record_failure(
    state: &AppState,
    connection: &ConnectionRow,
    reason: &str,
) -> AppResult<()> {
    sqlx::query(
        "update calendar_connections
            set status = 'error',
                error = $2,
                ics_etag = null,
                ics_last_modified = null,
                last_synced_at = now(),
                updated_at = now()
          where id = $1",
    )
    .bind(connection.id)
    .bind(reason)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a sync failure: {error}")))?;
    Ok(())
}

/// The start of the user's own local day, through `chrono-tz` and never a fixed offset (A6).
///
/// Read here rather than carried on the connection because the timezone belongs to the
/// `users` row and a copy on the connection would be a second source of truth that a student
/// changing their zone would not update.
///
/// `users.timezone` is `not null default 'Asia/Jakarta'` (the spec's default), so the fallback
/// is unreachable from a stored row; it exists so a deleted account mid-pass degrades to the
/// documented zone rather than to UTC, which would move a Jakarta class seven hours.
async fn local_day_start(state: &AppState, user_id: Uuid) -> AppResult<DateTime<Utc>> {
    let timezone: Option<String> =
        sqlx::query_scalar("select timezone from users where id = $1 and deleted_at is null")
            .bind(user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a timezone: {error}")))?;

    let timezone = timezone.unwrap_or_else(|| "Asia/Jakarta".to_string());
    Ok(crate::util::local_day_start(
        &timezone,
        state.clock.as_ref(),
    ))
}
