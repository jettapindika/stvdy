//! `POST /reminders`, `GET /reminders` and `POST /reminders/:id/ack`.
//!
//! The registration route is where the cross-device half of the phase's exit gate is decided,
//! and the whole of it is one key. Two phones with the same synced `tasks` row compute the
//! same due time to the second and both register a reminder; the key is
//! `(kind, subject, the user's local date)` and never the computed instant (A284), so the
//! second write is `on conflict (dedupe_key) do nothing` and the row keeps the **first**
//! `fire_at`. A key built from `fire_at` would produce two rows the moment the two devices
//! disagreed by a minute — which is the normal case, not the exceptional one.
//!
//! `applied` says whether this call was the one that stored the row, the Phase 9
//! `POST /sessions` shape and for the same reason: a client that retries after a lost
//! response can tell whether its write landed without re-reading (A285).
//!
//! `GET /reminders` is the in-app channel's feed, and it is **per device**: an ack records a
//! `reminder_deliveries` row keyed by `(reminder_id, 'web', device_id)`, so the same reminder
//! still shows on the student's other phone. A per-account ack would silence a reminder on a
//! device the student had not looked at yet.
//!
//! `crates/core/src/reminders.rs` owns the rules; this module is the shell that reads a
//! payload into them. The quiet-hours deferral is applied by the core function and not
//! re-derived here, so the two producers cannot disagree about where a reminder lands.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Duration, Utc};
use chrono_tz::Tz;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use stvdy_core::config::ReminderChannel;
use stvdy_core::reminders::{MAX_LOOKAHEAD_DAYS, ReminderKind, dedupe_key, defer_for_quiet_hours};

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tasks::Reject;

/// How far a client's clock may be behind the server's before the reminder is refused.
///
/// A client computing `fire_at` from synced data will be a little stale, and a phone whose
/// clock runs a minute fast is not an attacker. Five minutes is the same slack the session
/// validator grants a client's own clock (A204), and it is deliberately small: the window's
/// lower bound is what stops a client registering a reminder in the past, where it would fire
/// immediately and be delivered as though it were due.
pub const CLOCK_SKEW_MIN: i64 = 5;

/// The most reminders one feed response carries.
const FEED_LIMIT: i64 = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/reminders", get(feed).post(register))
        .route("/reminders/{id}/ack", post(ack))
}

/// A reminder a client computed from its own synced view.
#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct NewReminder {
    /// `task_due` or `event_soon`. `cards_due` is not registrable: it is day-granular and
    /// derived from the deck, so a client has nothing to compute it from that the server
    /// does not already have.
    pub kind: String,
    pub subject_id: Uuid,
    /// RFC3339, as the client computed it from synced data.
    pub fire_at: DateTime<Utc>,
}

/// The validator, shared with nothing else because nothing else writes a reminder.
///
/// `now` is passed in rather than read, so the window is a property of the arguments and a
/// test can put a client's clock on either side of the bound.
pub fn validate_new(payload: &NewReminder, now: DateTime<Utc>) -> Result<ReminderKind, Reject> {
    let Some(kind) = ReminderKind::from_str(&payload.kind) else {
        return Err(Reject::validation(format!(
            "kind must be {} or {}, got {}",
            ReminderKind::TaskDue.as_str(),
            ReminderKind::EventSoon.as_str(),
            payload.kind
        )));
    };
    if kind == ReminderKind::CardsDue {
        return Err(Reject::validation(
            "cards_due is derived from the deck and cannot be registered by a client",
        ));
    }

    // Bounded in both directions, and the messages name the bound: a client told only "invalid
    // fire_at" has no way to tell a clock that is five minutes fast from one that is five
    // years out.
    let earliest = now - Duration::minutes(CLOCK_SKEW_MIN);
    let latest = now + Duration::days(MAX_LOOKAHEAD_DAYS);
    if payload.fire_at < earliest {
        return Err(Reject::validation(format!(
            "fire_at must be no earlier than {CLOCK_SKEW_MIN} minutes ago, got {}",
            payload.fire_at.to_rfc3339()
        )));
    }
    if payload.fire_at > latest {
        return Err(Reject::validation(format!(
            "fire_at must be no later than {MAX_LOOKAHEAD_DAYS} days ahead, got {}",
            payload.fire_at.to_rfc3339()
        )));
    }

    Ok(kind)
}

/// The caller's own row in `user_settings`, read rather than created.
///
/// Deliberately **not** [`crate::settings::load_or_create`]: a reminder registration is not a
/// reason to create a settings row, and the insert would put a `change_log` entry in every
/// device's pull for an account that had merely registered a reminder. Absent settings mean
/// the column defaults, which is what a student who has never opened the screen has.
async fn reminder_settings(state: &AppState, user: &AuthUser) -> AppResult<(Option<(u8, u8)>, Tz)> {
    let row: Option<(Option<i32>, Option<i32>)> = sqlx::query_as(
        "select quiet_start_hour, quiet_end_hour from user_settings where user_id = $1",
    )
    .bind(user.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the quiet hours: {error}")))?;

    // A half-specified pair is impossible in the table (0028's paired check), so this only
    // has to decide what an absent pair means: no quiet hours.
    let quiet = row.and_then(|(start, end)| match (start, end) {
        (Some(start), Some(end)) => Some((
            u8::try_from(start).unwrap_or(22),
            u8::try_from(end).unwrap_or(7),
        )),
        _ => None,
    });

    // An unparseable zone falls back to UTC rather than failing the request, the same rule
    // `util::local_day_start` applies and for the same reason: the caller keeps the value the
    // user set, and a typo in `users.timezone` must not make reminders impossible to register.
    let tz: Tz = user.timezone.parse().unwrap_or(chrono_tz::UTC);
    Ok((quiet, tz))
}

/// `POST /reminders`: idempotent by the dedupe key.
pub async fn register(
    State(state): State<AppState>,
    user: VerifiedUser,
    Json(payload): Json<NewReminder>,
) -> AppResult<(StatusCode, Json<Value>)> {
    let now = state.clock.now();
    let kind = validate_new(&payload, now).map_err(Reject::into_app_error)?;

    // The subject must be the caller's own, and its kind must match the payload's: a
    // `task_due` naming an event id would register a reminder that fires at the wrong time
    // and is attributed to the wrong row, which no later layer can detect.
    let owned: bool = match kind {
        ReminderKind::TaskDue => sqlx::query_scalar(
            "select exists (select 1 from tasks
                             where id = $1 and user_id = $2 and deleted_at is null)",
        )
        .bind(payload.subject_id)
        .bind(user.user_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("checking a task: {error}")))?,
        ReminderKind::EventSoon => sqlx::query_scalar(
            "select exists (select 1 from events
                             where id = $1 and user_id = $2 and deleted_at is null)",
        )
        .bind(payload.subject_id)
        .bind(user.user_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("checking an event: {error}")))?,
        // Unreachable: the validator refuses it above, with a message that says why.
        ReminderKind::CardsDue => false,
    };
    if !owned {
        return Err(AppError::NotFound);
    }

    let (quiet, tz) = reminder_settings(&state, &user).await?;

    // The server re-derives the local date from `users.timezone` rather than trusting the
    // client's zone: a device whose zone is stale would otherwise register the same reminder
    // under a second key, which is exactly the duplicate the key exists to prevent.
    let local_date = payload.fire_at.with_timezone(&tz).date_naive();
    let subject_key = payload.subject_id.to_string();
    let key = dedupe_key(kind, &subject_key, local_date);

    // Quiet hours are applied here and not deferred to the sweep: the sweep only sees
    // `status = 'pending' and fire_at <= <the sweep's own clock instant>` — the injected
    // `state.clock.now()` and not the database's `now()`, so the two clocks never decide the
    // same question — and a reminder stored inside the quiet window would therefore be
    // delivered the moment the next tick ran rather than at the window's end.
    let deferral = defer_for_quiet_hours(payload.fire_at, payload.fire_at, tz, quiet);

    let inserted: Option<Uuid> = sqlx::query_scalar(
        "insert into reminders
            (user_id, kind, subject_id, subject_key, local_date, fire_at, dedupe_key,
             status, skip_reason, origin)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'client')
         on conflict (dedupe_key) do nothing
         returning id",
    )
    .bind(user.user_id)
    .bind(kind.as_str())
    .bind(payload.subject_id)
    .bind(&subject_key)
    .bind(local_date)
    .bind(deferral.fire_at)
    .bind(&key)
    .bind(if deferral.skip.is_some() {
        "skipped"
    } else {
        "pending"
    })
    .bind(deferral.skip.map(|reason| reason.as_str()))
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("registering a reminder: {error}")))?;

    let applied = inserted.is_some();
    // On a conflict the row already exists, so its id is read back: the client needs the id
    // either way, and a `null` on the second device would make "applied: false" and "no
    // reminder" indistinguishable.
    let reminder_id = match inserted {
        Some(id) => id,
        None => sqlx::query_scalar("select id from reminders where dedupe_key = $1")
            .bind(&key)
            .fetch_one(&state.pool)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("reading the existing reminder: {error}"))
            })?,
    };

    tracing::info!(
        user_id = %user.user_id,
        device_id = %user.device_id,
        reminder_id = %reminder_id,
        kind = kind.as_str(),
        applied,
        deferred = deferral.deferred,
        "reminder registered"
    );

    Ok((
        if applied {
            StatusCode::CREATED
        } else {
            StatusCode::OK
        },
        Json(json!({ "applied": applied, "reminder_id": reminder_id })),
    ))
}

/// One reminder as the in-app feed reports it.
///
/// The row's own columns and nothing derived: a deferral is visible as a `fire_at` later than
/// the subject's own time and as a `skip_reason`, both of which are here, and a `deferred`
/// boolean the server computed would be a second claim about the same fact that no column
/// backs.
#[derive(Debug, sqlx::FromRow, serde::Serialize)]
pub struct FeedRow {
    pub id: Uuid,
    pub kind: String,
    pub subject_id: Option<Uuid>,
    pub local_date: chrono::NaiveDate,
    pub fire_at: DateTime<Utc>,
    pub status: String,
    pub skip_reason: Option<String>,
}

/// `GET /reminders`: the in-app channel's feed.
///
/// Due, pending, and not yet acknowledged **by this device**. A `skipped` reminder stays in
/// the feed on purpose (A286): a reminder that silently never arrives is what the student
/// reports as "reminders are broken", and the reason is the only thing that distinguishes
/// "quiet hours moved it past its subject" from "the token is dead".
pub async fn feed(State(state): State<AppState>, user: VerifiedUser) -> AppResult<Json<Value>> {
    let now = state.clock.now();
    let rows: Vec<FeedRow> = sqlx::query_as(
        "select r.id, r.kind, r.subject_id, r.local_date, r.fire_at, r.status, r.skip_reason
           from reminders r
          where r.user_id = $1
            and r.status in ('pending', 'skipped')
            and r.fire_at <= $3
            and not exists (
                  select 1 from reminder_deliveries d
                   where d.reminder_id = r.id
                     and d.channel = 'web'
                     and d.device_id = $2)
          order by r.fire_at desc, r.id
          limit $4",
    )
    .bind(user.user_id)
    .bind(user.device_id)
    .bind(now)
    .bind(FEED_LIMIT)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing reminders: {error}")))?;

    Ok(Json(json!({ "reminders": rows })))
}

/// `POST /reminders/:id/ack`: one delivery row for the calling device, and a double tap is a
/// no-op.
///
/// `on conflict do nothing` against the partial unique index on
/// `(reminder_id, channel, device_id)` is what makes the retry safe, and the ack does **not**
/// change the reminder's status: another device has not seen it yet, and the sweep's claim is
/// what moves a reminder to `sent`.
pub async fn ack(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(id): Path<Uuid>,
) -> AppResult<StatusCode> {
    let owned: bool = sqlx::query_scalar(
        "select exists (select 1 from reminders where id = $1 and user_id = $2)",
    )
    .bind(id)
    .bind(user.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("checking a reminder: {error}")))?;
    if !owned {
        return Err(AppError::NotFound);
    }

    sqlx::query(
        "insert into reminder_deliveries (reminder_id, channel, device_id, status)
         values ($1, $2, $3, 'sent')
         on conflict do nothing",
    )
    .bind(id)
    .bind(channel_str(ReminderChannel::Web))
    .bind(user.device_id)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("acknowledging a reminder: {error}")))?;

    Ok(StatusCode::NO_CONTENT)
}

/// The `reminder_deliveries.channel` value for a channel.
///
/// One function rather than a `format!("{channel:?}").to_lowercase()` at each site: the
/// column is text and the enum is the config's, so a variant added to the matrix has to
/// answer here too rather than producing a delivery row no reader can match.
pub fn channel_str(channel: ReminderChannel) -> &'static str {
    match channel {
        ReminderChannel::Web => "web",
        ReminderChannel::Push => "push",
        ReminderChannel::Email => "email",
        ReminderChannel::Telegram => "telegram",
        ReminderChannel::Discord => "discord",
    }
}
