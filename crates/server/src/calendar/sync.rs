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
//!  * [`sync_google`] is a **cursor-driven pull**. Its rules are A236's — a pass that sent no
//!    cursor was full, a `410` clears the cursor and re-lists fully *in the same pass*, and the
//!    mode is written back so "an incremental sync that silently stopped being incremental" is
//!    a value a test asserts rather than an absence nobody sees — and A234's, where the decision
//!    for each remote event is [`policy::resolve`] rather than a rule this file remembers.
//!
//! Both halves share [`SyncOutcome`] and the upsert key.
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
use stvdy_core::calendar::policy::{self, LocalState, Origin, RemoteState, Winner};
use stvdy_core::entitlements::Feature;
use stvdy_core::traits::calendar::{CalendarError, RemoteEvent};

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
    /// The Google grant, encrypted at rest (`nonce || ciphertext`, the format
    /// `auth::oauth_state::encrypt_secret` writes). `None` for an ICS connection, which has no
    /// grant to refresh.
    ///
    /// Read here because the sync is the only reader: it decrypts this once per pass and hands
    /// the plaintext to `CalendarApi::refresh_access_token`. It is **not** in [`view`], which
    /// A249 fixes to a list with no token of any kind in it.
    pub refresh_token_enc: Option<Vec<u8>>,
}

/// The columns every read of a connection selects (A27: an explicit list, never `select *`).
const CONNECTION_COLUMNS: &str = "id, user_id, provider, account_email, sync_token, ics_url, \
     last_synced_at, status, error, ics_etag, ics_last_modified, study_calendar_id, \
     last_sync_was_full, refresh_token_enc";

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
                set last_synced_at = $2, updated_at = $2
              where id = $1",
        )
        .bind(connection.id)
        .bind(state.clock.now())
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
                last_synced_at = $6,
                updated_at = $6
          where id = $1",
    )
    .bind(connection.id)
    .bind(status)
    .bind(error.as_deref())
    .bind(fetched.etag.as_deref())
    .bind(fetched.last_modified.as_deref())
    .bind(state.clock.now())
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
                last_synced_at = $3,
                updated_at = $3
          where id = $1",
    )
    .bind(connection.id)
    .bind(reason)
    .bind(state.clock.now())
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

// ---------------------------------------------------------------------------------------
// The Google half (Task 7)
// ---------------------------------------------------------------------------------------

/// One pass over one Google connection: refresh the grant, list, decide each event with
/// [`policy::resolve`], then write the cursor and the mode (A236).
///
/// **The three failures, and why they are answered differently.** A `410` means the cursor
/// expired: the remedy is a *different request*, so this pass clears it and lists fully, and
/// it **succeeds**. An `Unauthorized` means the grant is gone: retrying cannot un-revoke it, so
/// the connection is marked `revoked` and the pass **succeeds** — a job that failed would retry
/// five times with backoff and record the same refusal five times (A247). A `Transport` failure
/// is returned as an error, and that is the only one a retry is worth
/// ([`CalendarError::is_retryable`]).
///
/// **The listing is the connection's own calendar.** `study_calendar_id` when the connect flow
/// found or created it, and `primary` when it could not: the cursor is one column on the
/// connection, so one pass is one listing, and a connection whose "Study Plan" calendar does
/// not exist yet still imports the student's own Google events rather than nothing.
///
/// **What this pass does not do.** It never writes to Google. `Winner::Local` — a mirrored row
/// of the student's own that the remote has not seen — is left exactly as it is, because the
/// outbound mirror is `CalendarApi::upsert_event` and is a write path of its own; a poll that
/// pushed on every tick would rewrite every mirrored row on every tick, which is the churn
/// A239 refuses for the ICS path.
pub async fn sync_google(state: &AppState, connection: &ConnectionRow) -> AppResult<SyncOutcome> {
    let Some(encrypted) = connection.refresh_token_enc.as_deref() else {
        // A row with no grant cannot be fixed by retrying, and one did not come from the
        // consent flow. Recorded rather than failed, for the same reason the ICS half records
        // a missing URL.
        record_failure(
            state,
            connection,
            "this Google connection has no stored grant",
        )
        .await?;
        return Ok(SyncOutcome::default());
    };
    let Some(api) = state.calendar.as_ref() else {
        // The deployment has no client: a connection row can outlive the configuration that
        // made it (or be written by a test). `state.calendar()`'s honest error is the reason,
        // and it is recorded rather than returned because no retry invents a client.
        let reason = state
            .calendar()
            .err()
            .map(|error| error.to_string())
            .unwrap_or_else(|| "no Google Calendar client is configured".to_string());
        record_failure(state, connection, &reason).await?;
        return Ok(SyncOutcome::default());
    };

    let started = std::time::Instant::now();
    let now = state.clock.now();
    let refresh_token = match crate::calendar::oauth::decrypt_refresh_token(state, encrypted) {
        Ok(token) => token,
        Err(error) => {
            // A row that does not decrypt cannot be fixed by retrying — a rotated key or a
            // truncated write is permanent for this row — so the reason is recorded and the
            // pass succeeds (A247), exactly as an unreachable ICS feed is.
            record_failure(state, connection, &error.to_string()).await?;
            return Ok(SyncOutcome::default());
        }
    };

    if let Err(error) = api.refresh_access_token(&refresh_token).await {
        return stop(state, connection, error).await;
    }

    let calendar_id = connection
        .study_calendar_id
        .as_deref()
        .filter(|id| !id.is_empty())
        .unwrap_or("primary");

    // A236's mode flag, decided by what this pass *sends* rather than by what it intends.
    let mut cursor = connection.sync_token.clone();
    let mut was_full = cursor.is_none();

    let page = match api.list_events(calendar_id, cursor.as_deref()).await {
        Ok(page) => page,
        Err(CalendarError::Gone) => {
            // The cursor is stale. `Gone` is a variant of its own precisely so this is a
            // different request rather than a retry: the same pass lists fully, and the pass
            // **succeeds** — the student's events are imported on this tick, not on the one
            // after five backoffs.
            tracing::info!(
                connection_id = %connection.id,
                "the Google sync cursor has expired; re-listing fully"
            );
            was_full = true;
            cursor = None;
            match api.list_events(calendar_id, None).await {
                Ok(page) => page,
                Err(error) => return stop(state, connection, error).await,
            }
        }
        Err(error) => return stop(state, connection, error).await,
    };

    let mut tx =
        state.pool.begin().await.map_err(|error| {
            AppError::Internal(anyhow::anyhow!("beginning a Google sync: {error}"))
        })?;
    let mut outcome = SyncOutcome {
        was_full,
        ..SyncOutcome::default()
    };
    let mut skipped = 0u32;

    for event in &page.events {
        // An event with no id cannot be keyed: `external_id` is the occurrence key (A231), and
        // an empty one would collide with every other id-less event on the partial unique
        // index.
        if event.id.is_empty() {
            skipped += 1;
            continue;
        }

        // One indexed lookup per event on `(connection_id, external_id)`. Reading the
        // connection's whole row set instead would trade a page of point lookups for a scan of
        // a calendar that may hold thousands of rows.
        let local = load_local(&mut tx, connection, &event.id).await?;

        // The origin is the **stored** `source` and not this pass's opinion (A234): a row we
        // mirrored (`source = 'app'`) is the student's, and a remote tombstone on it is
        // ignored; an adopted row follows the remote. A row we have never seen is the
        // remote's — `origin_of`'s "an unknown value is the calendar's", for the same reason.
        let origin = local
            .as_ref()
            .map(|row| policy::origin_of(&row.source))
            .unwrap_or(Origin::Google);

        // One comparison answers both of the policy's questions, because it is the only
        // evidence the schema holds: the stored `etag` is the remote's version of this row at
        // our last write, so a difference means one of the two sides moved since then.
        //
        // `changed` is that directly. `local_dirty` is the same value read the other way, and
        // it is the honest reading rather than an invented one: for an `app` row a difference
        // means the local copy may carry an edit the remote has not seen (the policy's own
        // definition), and for a `google` row the policy ignores the flag. The answer is the
        // same either way here, because this pass writes nothing for an app row.
        let moved = local
            .as_ref()
            .map(|row| row.etag.as_deref() != Some(event.etag.as_str()))
            .unwrap_or(true);
        let local_state = match &local {
            Some(row) => LocalState {
                exists: !row.deleted,
                local_dirty: moved,
            },
            None => LocalState::absent(),
        };
        let remote_state = RemoteState {
            changed: moved,
            deleted: event.is_cancelled(),
        };

        match policy::resolve(origin, local_state, remote_state) {
            // Rule 3's second half: a tombstone on an adopted row is a real deletion. It
            // arrives **only** through an incremental listing, which is why the client sends
            // `showDeleted=true` on every listing rather than only on the ones it remembers to.
            Winner::Remote if event.is_cancelled() => {
                if tombstone_google_event(&mut tx, connection, &event.id, now).await? {
                    outcome.deleted += 1;
                }
            }
            Winner::Remote => {
                // `events.starts_at` and `ends_at` are `not null`, and the trait's `Option` is
                // there so nothing invents one: an all-day event arrives as a `date` with no
                // instant in it, and midnight UTC would place a Jakarta event at 07:00 local.
                // Counted and logged rather than refused, because one unplaceable event must
                // not cost the student the other two hundred on the page.
                let (Some(starts_at), Some(ends_at)) = (event.start, event.end) else {
                    skipped += 1;
                    continue;
                };
                if upsert_google_event(&mut tx, connection, event, starts_at, ends_at).await? {
                    outcome.inserted += 1;
                } else {
                    outcome.updated += 1;
                }
            }
            // `Local`: the local row stands, and pushing it is the outbound mirror's job
            // rather than this pass's (see this function's documentation). `Neither`: the two
            // agree, or neither side has anything to say.
            Winner::Local | Winner::Neither => {}
        }
    }

    // The cursor and the mode, written in the **same transaction** as the events: a pass that
    // stored the events and then failed to advance the cursor would re-read the same delta
    // forever, and one that advanced the cursor without storing them would lose them.
    let next_cursor = match page.next_sync_token.clone() {
        Some(token) => Some(token),
        None => {
            // The trait's own note: `next_sync_token` is `None` for a *successful* listing only
            // when Google's side is wrong. Storing `null` would make the next pass full without
            // anyone noticing, which is A236's exact invisible failure. Keeping the cursor this
            // pass sent means the next pass re-reads the same delta — idempotent on
            // `(connection_id, external_id)` — and a cursor Google has since expired heals
            // through the `Gone` path above.
            tracing::warn!(
                connection_id = %connection.id,
                "Google's listing carried no nextSyncToken; keeping the stored cursor"
            );
            cursor
        }
    };

    sqlx::query(
        "update calendar_connections
            set sync_token = $2,
                last_sync_was_full = $3,
                last_synced_at = $4,
                status = 'connected',
                error = null,
                updated_at = $4
          where id = $1",
    )
    .bind(connection.id)
    .bind(next_cursor.as_deref())
    .bind(was_full)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("recording a Google sync: {error}")))?;

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing a Google sync: {error}"))
    })?;

    tracing::info!(
        connection_id = %connection.id,
        provider = "google",
        calendar_id,
        was_full,
        inserted = outcome.inserted,
        updated = outcome.updated,
        deleted = outcome.deleted,
        skipped,
        latency_ms = started.elapsed().as_millis() as u64,
        "the Google calendar was synced"
    );
    Ok(outcome)
}

/// What a failed Google call means for the connection.
///
/// `Unauthorized` marks it `revoked` and the pass **succeeds**: retrying cannot un-revoke a
/// grant, and the student's remedy is to reconnect (A247). A retryable failure is returned as
/// an error so the worker's own backoff runs. Anything else — a provider answer this code
/// cannot use, a missing token — is recorded on the connection and the pass succeeds, exactly
/// as an unreachable ICS feed is, because the poll picks `error` rows back up on the next tick
/// while a failed job would retry the same unanswerable request five times.
async fn stop(
    state: &AppState,
    connection: &ConnectionRow,
    error: CalendarError,
) -> AppResult<SyncOutcome> {
    match error {
        CalendarError::Unauthorized(reason) => {
            let now = state.clock.now();
            sqlx::query(
                "update calendar_connections
                    set status = 'revoked',
                        error = $2,
                        last_synced_at = $3,
                        updated_at = $3
                  where id = $1",
            )
            .bind(connection.id)
            .bind(&reason)
            .bind(now)
            .execute(&state.pool)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("revoking a Google connection: {error}"))
            })?;
            tracing::info!(
                connection_id = %connection.id,
                %reason,
                "the Google grant was refused; the connection is revoked"
            );
            Ok(SyncOutcome::default())
        }
        // The trait's own predicate rather than a match on `Transport`: it is what says which
        // failures a retry is worth, and a variant added later is answered here without this
        // file changing.
        other if other.is_retryable() => Err(AppError::Provider(other.to_string())),
        other => {
            record_failure(state, connection, &other.to_string()).await?;
            tracing::warn!(
                connection_id = %connection.id,
                error = %other,
                "the Google calendar call failed"
            );
            Ok(SyncOutcome::default())
        }
    }
}

/// One local row, as [`policy::resolve`] needs it.
#[derive(Debug, Clone, sqlx::FromRow)]
struct LocalRow {
    /// `events.source`, which decides the origin.
    source: String,
    /// The remote's version of this row at our last write, or `None` if we have never written
    /// one (a row an earlier build left behind).
    etag: Option<String>,
    /// `deleted_at is not null`.
    deleted: bool,
}

/// The local row for one remote event, or `None` if this connection has never adopted it.
async fn load_local(
    tx: &mut PgConnection,
    connection: &ConnectionRow,
    external_id: &str,
) -> AppResult<Option<LocalRow>> {
    sqlx::query_as(
        "select source, etag, deleted_at is not null as deleted
           from events
          where connection_id = $1 and external_id = $2",
    )
    .bind(connection.id)
    .bind(external_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading an adopted event: {error}")))
}

/// The upsert for an adopted Google event.
///
/// The key is `(connection_id, external_id)` (A231) — Google's own opaque id, verbatim — so a
/// re-poll updates the same row rather than inserting a second copy of every event, which is
/// what makes a poll safe to repeat. `xmax = 0` is what tells an insert from an update without
/// a second round trip, exactly as [`upsert_occurrence`] does it.
///
/// The `do update` clause deliberately does **not** touch `source`: a row that resolved to
/// [`Winner::Remote`] is a Google row (the policy answers `Winner::Local` or `Winner::Neither`
/// for every other origin), and rewriting `source` here would silently reclassify a student's
/// own row if that ever stopped being true.
async fn upsert_google_event(
    tx: &mut PgConnection,
    connection: &ConnectionRow,
    event: &RemoteEvent,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
) -> AppResult<bool> {
    let inserted: bool = sqlx::query_scalar(
        "insert into events
            (id, user_id, kind, title, starts_at, ends_at, external_id, etag, connection_id,
             source)
         values ($1, $2, 'external', $3, $4, $5, $6, $7, $8, 'google')
         on conflict (connection_id, external_id) where external_id is not null
         do update set title = excluded.title,
                       starts_at = excluded.starts_at,
                       ends_at = excluded.ends_at,
                       etag = excluded.etag,
                       deleted_at = null
         returning xmax = 0",
    )
    .bind(Uuid::now_v7())
    .bind(connection.user_id)
    .bind(&event.summary)
    .bind(starts_at)
    .bind(ends_at)
    .bind(&event.id)
    .bind(&event.etag)
    .bind(connection.id)
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("importing a Google event: {error}")))?;

    Ok(inserted)
}

/// A remote cancellation, honoured as a local tombstone.
///
/// A soft delete rather than a `delete`: `events` is `+sync`, and the tombstone is what tells
/// every device to drop the row (the trigger logs it as a `delete`). It only ever touches a row
/// that is still live, so a cancellation that arrives twice counts once.
async fn tombstone_google_event(
    tx: &mut PgConnection,
    connection: &ConnectionRow,
    external_id: &str,
    now: DateTime<Utc>,
) -> AppResult<bool> {
    let affected = sqlx::query(
        "update events
            set deleted_at = $3
          where connection_id = $1 and external_id = $2 and deleted_at is null",
    )
    .bind(connection.id)
    .bind(external_id)
    .bind(now)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("tombstoning a Google event: {error}")))?;

    Ok(affected.rows_affected() > 0)
}
