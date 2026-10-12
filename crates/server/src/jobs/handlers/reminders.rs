//! `send_reminder` and `send_push`: derive once, claim once, deliver once.
//!
//! Two kinds because the roadmap's § 8 job list is fixed and has exactly those two (A287), and
//! they do different things:
//!
//!  * `send_reminder` is **per account** and is the *producer*. It derives the account's
//!    reminders from `tasks`/`events`/`user_settings` through the pure engine, registers the
//!    ones that do not exist yet (`on conflict (dedupe_key) do nothing`), then claims the ones
//!    that are now due and enqueues one `send_push` per claimed reminder.
//!  * `send_push` is **per reminder** and is the *delivery*. It fans out over the account's
//!    enabled, available channels, records one `reminder_deliveries` row per attempt, and
//!    returns a transient failure only when **every** channel failed.
//!
//! The claim is what makes a reminder fire once. Two workers, a retried job and a second
//! instance all see the same `status = 'pending'` row, and
//! `update reminders set status = 'sent' where id = $1 and status = 'pending' returning id`
//! lets exactly one of them win; the loser enqueues nothing. The `send_push` job's own
//! `dedupe_key` (`send_push:<reminder_id>`) is the second layer, and the partial unique index
//! on `reminder_deliveries` is the third — so a double-enqueue is a no-op rather than a second
//! notification.
//!
//! A malformed payload is a **permanent** failure and not a transient one. `{"reminder_id"}`
//! missing or unparseable will never become well-formed, and a transient failure would retry
//! it five times with exponential backoff and then park it — a retry storm for a message that
//! was never going to arrive.

use chrono::{DateTime, Duration, Utc};
use sqlx::PgPool;
use uuid::Uuid;

use stvdy_core::config::ReminderChannel;
use stvdy_core::reminders::{
    MAX_LOOKAHEAD_DAYS, PlannedReminder, ReminderKind, ReminderSettings, SkipReason, Subject, plan,
};
use stvdy_core::traits::push::{PushError, PushMessage};

use crate::jobs::{ClaimedJob, Failure};
use crate::reminders::channel_str;
use crate::state::AppState;

/// Every channel a student may enable, in the matrix's order.
///
/// All five, not only the three with transports: `Telegram` and `Discord` are granted by the
/// plan matrix and selectable on the settings screen, so they must be **attempted and
/// recorded** with `reason = 'channel_not_implemented'` rather than silently skipped. A Power
/// user who selected one and gets silence has no way to tell that from a broken reminder.
///
/// The order is the matrix's (`web, push, email, telegram, discord`), so the delivery rows and
/// the settings screen agree on what the list is.
const ALL_CHANNELS: [ReminderChannel; 5] = [
    ReminderChannel::Web,
    ReminderChannel::Push,
    ReminderChannel::Email,
    ReminderChannel::Telegram,
    ReminderChannel::Discord,
];

/// Whether this deployment has a transport for a channel (A288).
///
/// The three implemented ones, and nothing else: the two chat channels are deferred with a
/// named revisit trigger, and this is the single place that decides, so `channels_available`
/// and the delivery path cannot disagree about which channels fire.
fn has_transport(state: &AppState, channel: ReminderChannel) -> bool {
    match channel {
        ReminderChannel::Web | ReminderChannel::Email => true,
        ReminderChannel::Push => state.push.is_configured(),
        ReminderChannel::Telegram | ReminderChannel::Discord => false,
    }
}

/// One channel's outcome for one reminder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeliveryOutcome {
    pub channel: String,
    pub device_id: Option<Uuid>,
    pub status: &'static str,
    pub reason: Option<String>,
}

/// One reminder as the delivery path reads it.
#[derive(Debug, Clone, sqlx::FromRow)]
pub struct ReminderRow {
    pub id: Uuid,
    pub user_id: Uuid,
    pub kind: String,
    pub subject_key: String,
    pub fire_at: DateTime<Utc>,
}

/// `send_reminder`: one account's due reminders, derived and claimed (A287).
pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let user_id = job.user_id()?;
    let now = state.clock.now();

    let Some(account) = load_account(state, user_id).await? else {
        // The account is gone or soft-deleted: nothing to remind, and nothing to retry.
        tracing::info!(%user_id, "send_reminder: no live account");
        return Ok(());
    };

    let planned = plan(now, &account.settings, &account.subjects);
    let mut registered = 0u64;
    for reminder in &planned {
        if register(state, user_id, reminder).await? {
            registered += 1;
        }
    }

    // The claim is the delivery gate, and it reads the *table* rather than the plan: a
    // reminder a client registered through `POST /reminders` is due on exactly the same terms
    // as one this sweep derived, and there is one claim path for both.
    let due: Vec<Uuid> = sqlx::query_scalar(
        "select id from reminders
          where user_id = $1 and status = 'pending' and fire_at <= $2
          order by fire_at asc, id
          limit $3",
    )
    .bind(user_id)
    .bind(now)
    .bind(CLAIM_BATCH)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("listing due reminders: {error}")))?;

    let mut claimed = 0u64;
    for reminder_id in due {
        if !claim_reminder(&state.pool, reminder_id, now)
            .await
            .map_err(|error| Failure::transient(format!("claiming a reminder: {error}")))?
        {
            // Another worker got it between the select and the claim, which is exactly what
            // the claim is for. Not an error.
            continue;
        }
        crate::jobs::enqueue(
            &state.pool,
            &state.jobs,
            "send_push",
            serde_json::json!({ "reminder_id": reminder_id }),
            Some(&format!("send_push:{reminder_id}")),
        )
        .await
        .map_err(|error| Failure::transient(format!("enqueuing a delivery: {error}")))?;
        claimed += 1;
    }

    if registered > 0 || claimed > 0 {
        tracing::info!(%user_id, registered, claimed, "send_reminder derived and claimed");
    }
    Ok(())
}

/// How many reminders one pass claims. Bounded so one account's backlog cannot hold the
/// worker, and idempotent so a second pass covers the rest.
const CLAIM_BATCH: i64 = 200;

/// The account's reminder inputs, or `None` when the account is gone.
struct Account {
    settings: ReminderSettings,
    subjects: Vec<Subject>,
}

/// The raw `users`/`user_settings` row behind an [`Account`]: the timezone, the three
/// quiet/study hours (all nullable), and the channel list the column defaults to `["web"]`.
type AccountRow = (
    String,
    Option<i32>,
    Option<i32>,
    Option<i32>,
    sqlx::types::Json<Vec<ReminderChannel>>,
);

async fn load_account(state: &AppState, user_id: Uuid) -> Result<Option<Account>, Failure> {
    let row: Option<AccountRow> = sqlx::query_as(
        "select u.timezone,
                    s.quiet_start_hour,
                    s.quiet_end_hour,
                    s.study_start_hour,
                    coalesce(s.reminder_channels, '[\"web\"]'::jsonb)
               from users u
               left join user_settings s on s.user_id = u.id
              where u.id = $1 and u.deleted_at is null",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the account: {error}")))?;

    let Some((timezone, quiet_start, quiet_end, study_start, channels)) = row else {
        return Ok(None);
    };

    let now = state.clock.now();
    // Unparseable zone falls back to UTC rather than failing the job, the same rule
    // `util::local_day_start` and `reminders::reminder_settings` apply: a typo in
    // `users.timezone` must not stop every reminder for the account.
    let tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let quiet = match (quiet_start, quiet_end) {
        (Some(start), Some(end)) => Some((
            u8::try_from(start).unwrap_or(22),
            u8::try_from(end).unwrap_or(7),
        )),
        _ => None,
    };

    let horizon = now + Duration::days(MAX_LOOKAHEAD_DAYS);
    // Two queries rather than one union: a task's due instant and an event's start are
    // different columns on different tables, and a union would make the `done` rule (a
    // completed task) look like it applies to both.
    let tasks: Vec<(Uuid, DateTime<Utc>)> = sqlx::query_as(
        "select id, due_at from tasks
          where user_id = $1 and deleted_at is null and status <> 'done'
            and due_at is not null and due_at > $2 and due_at <= $3
          order by due_at, id",
    )
    .bind(user_id)
    .bind(now)
    .bind(horizon)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("listing tasks: {error}")))?;

    let events: Vec<(Uuid, DateTime<Utc>)> = sqlx::query_as(
        "select id, starts_at from events
          where user_id = $1 and deleted_at is null and starts_at > $2 and starts_at <= $3
          order by starts_at, id",
    )
    .bind(user_id)
    .bind(now)
    .bind(horizon)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("listing events: {error}")))?;

    let mut subjects: Vec<Subject> = tasks
        .into_iter()
        .map(|(id, at)| Subject {
            kind: ReminderKind::TaskDue,
            id,
            at: Some(at),
            done: false,
        })
        .chain(events.into_iter().map(|(id, at)| Subject {
            kind: ReminderKind::EventSoon,
            id,
            at: Some(at),
            done: false,
        }))
        .collect();
    subjects.sort_by(|left, right| left.id.cmp(&right.id));

    let cards_due = due_card_count(state, user_id).await?;

    Ok(Some(Account {
        settings: ReminderSettings {
            tz,
            quiet,
            channels: channels.0,
            study_start_hour: u8::try_from(study_start.unwrap_or(7)).unwrap_or(7),
            cards_due,
        },
        subjects,
    }))
}

/// How many cards the account's own deck says are due, as an `anyhow` error.
///
/// The dashboard and the reminder engine must not disagree about this number: it is the same
/// count, and a second query in `dashboard.rs` would be a second answer to "how many cards are
/// due". The reminder handler keeps its own `Failure`-typed wrapper because a job needs to say
/// whether a retry could help, which is a distinction the dashboard has no use for.
pub async fn due_cards_for_user(state: &AppState, user_id: Uuid) -> Result<i64, anyhow::Error> {
    due_card_count(state, user_id)
        .await
        .map_err(|failure| anyhow::anyhow!("{}", failure.message))
}

/// How many cards the account's own deck says are due.
///
/// The same predicate `planner::due_cards_for` uses for one goal's material, minus the source
/// scoping: a reminder is account-level, so every deck counts. Both are a count over
/// `flashcards` joined through `decks`, which is where the ownership lives.
async fn due_card_count(state: &AppState, user_id: Uuid) -> Result<i64, Failure> {
    let now = state.clock.now();
    sqlx::query_scalar(
        "select count(*)::bigint from flashcards f
           join decks d on d.id = f.deck_id
          where f.user_id = $1 and f.deleted_at is null
            and d.deleted_at is null
            and f.due_at <= $2",
    )
    .bind(user_id)
    .bind(now)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("counting due cards: {error}")))
}

/// Registers one planned reminder, or leaves the existing row alone.
///
/// `on conflict (dedupe_key) do nothing` is the second of A285's three layers: a client that
/// already registered this reminder through `POST /reminders` keeps its row and its `fire_at`,
/// and a second sweep is a no-op. Returns whether this call wrote the row.
async fn register(
    state: &AppState,
    user_id: Uuid,
    reminder: &PlannedReminder,
) -> Result<bool, Failure> {
    let inserted: Option<Uuid> = sqlx::query_scalar(
        "insert into reminders
            (user_id, kind, subject_id, subject_key, local_date, fire_at, dedupe_key,
             status, skip_reason, origin)
         values ($1, $2, $3, $4, $5, $6, $7, $8, $9, 'server')
         on conflict (dedupe_key) do nothing
         returning id",
    )
    .bind(user_id)
    .bind(reminder.kind.as_str())
    .bind(reminder.subject_id)
    .bind(&reminder.subject_key)
    .bind(reminder.local_date)
    .bind(reminder.fire_at)
    .bind(&reminder.dedupe_key)
    .bind(if reminder.skip.is_some() {
        "skipped"
    } else {
        "pending"
    })
    .bind(reminder.skip.map(SkipReason::as_str))
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("registering a reminder: {error}")))?;
    Ok(inserted.is_some())
}

/// The atomic claim: `update … set status='sent' where id = $1 and status='pending'
/// returning id`. Zero rows means another worker got it.
///
/// `now` is the **injected** clock's instant, passed in rather than read from the database, and
/// that is not a detail. The sweep decides a reminder is due by comparing `fire_at` against
/// `state.clock.now()`, so a claim stamped with `now()` would record a `sent_at` from a
/// different clock than the one that made the decision: the two agree in production and diverge
/// the moment a test pins the clock, which makes "the reminder was sent at the instant the
/// sweep ran" impossible to assert — the same two-clock split a sibling found in
/// `calendar_pass`. It is the same rule `schedule.rs` states for the compaction cutoff.
pub async fn claim_reminder(
    pool: &PgPool,
    id: Uuid,
    now: DateTime<Utc>,
) -> crate::error::AppResult<bool> {
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "update reminders set status = 'sent', sent_at = $2
          where id = $1 and status = 'pending'
          returning id",
    )
    .bind(id)
    .bind(now)
    .fetch_optional(pool)
    .await
    .map_err(|error| {
        crate::error::AppError::Internal(anyhow::anyhow!("claiming a reminder: {error}"))
    })?;
    Ok(claimed.is_some())
}

/// `send_push`: one reminder over every enabled, available channel (A287).
pub async fn deliver(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let Some(reminder_id) = job
        .payload
        .get("reminder_id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
    else {
        // Permanent: a payload with no readable id will never have one, and a transient
        // failure would retry it five times and park a message that cannot arrive.
        return Err(Failure::permanent("payload has no valid reminder_id"));
    };

    let reminder: Option<ReminderRow> = sqlx::query_as(
        "select id, user_id, kind, subject_key, fire_at from reminders where id = $1",
    )
    .bind(reminder_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the reminder: {error}")))?;

    let Some(reminder) = reminder else {
        // The reminder is gone: the account was deleted, or the row was pruned. Nothing to
        // deliver and nothing a retry could find.
        tracing::info!(%reminder_id, "send_push: the reminder no longer exists");
        return Ok(());
    };

    // The producer's own copy when the payload carries one, which is how a producer that is
    // not the core engine (A300's renewal reminders) owns its own text. The engine's own
    // producer sends only an id, and the copy is derived from the row.
    let title = job
        .payload
        .get("title")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| default_title(&reminder));
    let body = job
        .payload
        .get("body")
        .and_then(|value| value.as_str())
        .map(str::to_string)
        .unwrap_or_else(|| default_body(&reminder));

    let outcomes = deliver_one(state, &reminder, &title, &body)
        .await
        .map_err(|error| Failure::transient(format!("delivering a reminder: {error}")))?;

    // `Ok` unless **every** channel failed, so one bad channel cannot re-send the ones that
    // worked. A channel with no transport is `skipped`, which is neither a success nor a
    // failure: it is a status the student can see.
    let attempted = outcomes
        .iter()
        .filter(|outcome| outcome.status != "skipped")
        .count();
    let failed = outcomes
        .iter()
        .filter(|outcome| outcome.status == "failed")
        .count();

    if attempted > 0 && failed == attempted {
        return Err(Failure::transient(format!(
            "every channel failed for reminder {reminder_id}"
        )));
    }

    tracing::info!(
        %reminder_id,
        channels = outcomes.len(),
        failed,
        "send_push delivered"
    );
    Ok(())
}

/// Every channel's outcome for one reminder, so a partial failure is a record and not a
/// retry of the channels that worked.
///
/// The text is a parameter rather than derived here, so a producer that is not the engine can
/// own its own copy — the payload's `title`/`body` when it carries them, and the row's own
/// kind when it does not.
pub async fn deliver_one(
    state: &AppState,
    reminder: &ReminderRow,
    title: &str,
    body: &str,
) -> crate::error::AppResult<Vec<DeliveryOutcome>> {
    let (channels, email): (sqlx::types::Json<Vec<ReminderChannel>>, Option<String>) =
        sqlx::query_as(
            "select coalesce(s.reminder_channels, '[\"web\"]'::jsonb), u.email
           from users u
           left join user_settings s on s.user_id = u.id
          where u.id = $1",
        )
        .bind(reminder.user_id)
        .fetch_one(&state.pool)
        .await
        .map_err(|error| {
            crate::error::AppError::Internal(anyhow::anyhow!(
                "loading the delivery settings: {error}"
            ))
        })?;

    // Deduped and ordered: the settings row is a `jsonb` array a client wrote, so it can
    // legitimately carry the same channel twice, and two `push` entries would attempt the same
    // device twice and collide on the deliveries index. A `Vec` rather than a set because
    // `ReminderChannel` is the config's own enum and carries no `Ord`; the membership tests
    // below are over three values, so the difference is not worth a derive on a shared type.
    let enabled: Vec<ReminderChannel> = ALL_CHANNELS
        .into_iter()
        .filter(|channel| channels.0.contains(channel))
        .collect();

    // The channels that are already *settled*: `sent` (it went) or `skipped` (it had nowhere
    // to go). Those are not attempted again, which is what makes a re-run of this job deliver
    // once — the same reminder claimed twice, a retry after a lost `mark_done`, or a second
    // instance. A `failed` channel is deliberately **not** settled: it is the one case a retry
    // is for, and re-attempting it updates its own row rather than adding a second.
    let settled: Vec<String> = sqlx::query_scalar(
        "select distinct channel from reminder_deliveries
          where reminder_id = $1 and status in ('sent', 'skipped')",
    )
    .bind(reminder.id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| {
        crate::error::AppError::Internal(anyhow::anyhow!("listing the settled deliveries: {error}"))
    })?
    .into_iter()
    .collect();

    let mut outcomes = Vec::new();
    for channel in enabled {
        if settled.iter().any(|c| c == channel_str(channel)) {
            continue;
        }
        if !has_transport(state, channel) {
            // A288: granted by the matrix, selectable, and with no transport. Recorded so a
            // Power user who chose one sees a status rather than silence.
            tracing::warn!(
                channel = channel_str(channel),
                reminder_id = %reminder.id,
                "a reminder channel has no transport"
            );
            outcomes.push(
                record(
                    state,
                    reminder.id,
                    channel,
                    None,
                    "skipped",
                    Some("channel_not_implemented".to_string()),
                    None,
                )
                .await?,
            );
            continue;
        }
        let outcome = match channel {
            ReminderChannel::Web => {
                record(
                    state,
                    reminder.id,
                    ReminderChannel::Web,
                    None,
                    "sent",
                    None,
                    None,
                )
                .await?
            }
            ReminderChannel::Email => match email.as_deref() {
                Some(to) => send_email(state, reminder, to, title, body).await?,
                // Unreachable: `users.email` is `not null`. Named rather than unwrapped so a
                // future nullable column is a recorded skip instead of a panic in a job.
                None => {
                    record(
                        state,
                        reminder.id,
                        ReminderChannel::Email,
                        None,
                        "skipped",
                        Some("no_email_address".to_string()),
                        None,
                    )
                    .await?
                }
            },
            ReminderChannel::Push => send_push(state, reminder, title, body).await?,
            // Unreachable: `has_transport` refuses both above, and it is the one place that
            // decides. Named rather than `unreachable!()` because a panic in a job is a
            // worker-wide failure for a decision that has a safe answer.
            ReminderChannel::Telegram | ReminderChannel::Discord => {
                record(
                    state,
                    reminder.id,
                    channel,
                    None,
                    "skipped",
                    Some("channel_not_implemented".to_string()),
                    None,
                )
                .await?
            }
        };
        outcomes.push(outcome);
    }

    Ok(outcomes)
}

/// One delivery row, and the outcome it represents.
///
/// `on conflict do nothing` against the partial unique indexes is what makes a retried
/// delivery record one attempt rather than two; the row is then read back so the outcome
/// reports what is *stored* rather than what this call intended.
#[allow(clippy::too_many_arguments)]
async fn record(
    state: &AppState,
    reminder_id: Uuid,
    channel: ReminderChannel,
    device_id: Option<Uuid>,
    status: &'static str,
    reason: Option<String>,
    error: Option<String>,
) -> crate::error::AppResult<DeliveryOutcome> {
    sqlx::query(
        "insert into reminder_deliveries
            (reminder_id, channel, device_id, status, reason, error)
         values ($1, $2, $3, $4, $5, $6)
         on conflict do nothing",
    )
    .bind(reminder_id)
    .bind(channel_str(channel))
    .bind(device_id)
    .bind(status)
    .bind(reason.as_deref())
    .bind(error.as_deref())
    .execute(&state.pool)
    .await
    .map_err(|error| {
        crate::error::AppError::Internal(anyhow::anyhow!("recording a delivery: {error}"))
    })?;

    Ok(DeliveryOutcome {
        channel: channel_str(channel).to_string(),
        device_id,
        status,
        reason,
    })
}

/// The in-app feed *is* the web delivery, so this records that the reminder is now visible
/// rather than sending anything.
///
/// The row has `device_id = null`, which is deliberately a different row from a device's ack:
/// the account-level row says "the feed carries this", and each device's ack says "this device
/// has seen it". The partial unique indexes keep the two apart, which is why an ack on one
/// phone does not silence the reminder on another.
async fn send_email(
    state: &AppState,
    reminder: &ReminderRow,
    to: &str,
    title: &str,
    body: &str,
) -> crate::error::AppResult<DeliveryOutcome> {
    use stvdy_core::traits::mailer::Email;

    let message = Email {
        to: to.to_string(),
        subject: title.to_string(),
        text: body.to_string(),
        html: None,
    };
    match state.mailer.send(message).await {
        Ok(()) => {
            record(
                state,
                reminder.id,
                ReminderChannel::Email,
                None,
                "sent",
                None,
                None,
            )
            .await
        }
        Err(error) => {
            record(
                state,
                reminder.id,
                ReminderChannel::Email,
                None,
                "failed",
                None,
                Some(error.to_string()),
            )
            .await
        }
    }
}

/// Push to the single most recently active device with a token (A285).
///
/// One device, not all of them: the spec's rule is "deliver to the most recently active
/// device, **or** dedupe by reminder id", and the plan takes both — the first is a delivery
/// rule, and the second is the fallback when a device has no token (the payload carries
/// `reminder_id`, and the client keeps the ids it has already shown).
///
/// A dead token is **deleted** and the next device is tried, because a dead token is not an
/// outage: the whole reason `PushError::Unregistered` is separate is that deleting a token
/// whose phone was merely asleep would silently stop that device's reminders forever.
async fn send_push(
    state: &AppState,
    reminder: &ReminderRow,
    title: &str,
    body: &str,
) -> crate::error::AppResult<DeliveryOutcome> {
    let candidates: Vec<(Uuid, String, String)> = sqlx::query_as(
        "select d.id, t.provider, t.token
           from devices d
           join push_tokens t on t.device_id = d.id
          where d.user_id = $1 and d.revoked_at is null
          order by d.last_seen_at desc nulls last, d.id",
    )
    .bind(reminder.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| {
        crate::error::AppError::Internal(anyhow::anyhow!("listing push candidates: {error}"))
    })?;

    let mut last_error: Option<String> = None;
    let mut tried = false;

    for (device_id, provider, token) in candidates {
        // A device whose vendor has no transport is skipped rather than attempted: the
        // settings screen reports that channel unavailable (A288), and a device on that
        // platform simply cannot be reached from this deployment.
        let Some(transport) = state.push.for_provider(&provider) else {
            continue;
        };
        tried = true;

        let mut data = std::collections::BTreeMap::new();
        data.insert("reminder_id".to_string(), reminder.id.to_string());
        data.insert("kind".to_string(), reminder.kind.clone());
        let message = PushMessage {
            title: title.to_string(),
            body: body.to_string(),
            data,
            // A reminder that fires once should read once: the collapse key replaces an
            // undelivered notification with the same id rather than stacking a second.
            collapse_key: Some(format!("reminder-{}", reminder.id)),
            badge: None,
        };

        match transport.send(&token, message).await {
            Ok(()) => {
                return record(
                    state,
                    reminder.id,
                    ReminderChannel::Push,
                    Some(device_id),
                    "sent",
                    None,
                    None,
                )
                .await;
            }
            Err(PushError::Unregistered) => {
                // The only failure that deletes a row. Logged because a device silently
                // dropping off the push list is otherwise invisible.
                sqlx::query("delete from push_tokens where device_id = $1")
                    .bind(device_id)
                    .execute(&state.pool)
                    .await
                    .map_err(|error| {
                        crate::error::AppError::Internal(anyhow::anyhow!(
                            "deleting a dead push token: {error}"
                        ))
                    })?;
                tracing::warn!(%device_id, %provider, "a push token was unregistered; trying the next device");
                last_error = Some("the push token is no longer registered".to_string());
            }
            Err(error) => {
                // A transport failure says nothing about the token, so the row stays and the
                // next device is tried. Recorded as a failure for *this* device.
                last_error = Some(error.to_string());
            }
        }
    }

    let (status, reason, error) = match last_error {
        Some(error) => ("failed", None, Some(error)),
        // No device had a usable token. `skipped` rather than `failed`: nothing is wrong with
        // the reminder, and a failure here would make `send_push` retry a message that has
        // nowhere to go.
        None if tried => ("failed", None, Some("the push was rejected".to_string())),
        None => ("skipped", Some("no_push_token".to_string()), None),
    };

    record(
        state,
        reminder.id,
        ReminderChannel::Push,
        None,
        status,
        reason,
        error,
    )
    .await
}

/// The default copy for a reminder the engine derived.
fn default_title(reminder: &ReminderRow) -> String {
    match ReminderKind::from_str(&reminder.kind) {
        Some(ReminderKind::TaskDue) => "A task is due soon".to_string(),
        Some(ReminderKind::EventSoon) => "An event starts soon".to_string(),
        Some(ReminderKind::CardsDue) => "You have cards to review".to_string(),
        // A producer that is not the engine (A300's renewals) supplies its own copy through
        // the payload; this is the fallback for one that did not.
        None => "Reminder".to_string(),
    }
}

fn default_body(reminder: &ReminderRow) -> String {
    match ReminderKind::from_str(&reminder.kind) {
        Some(ReminderKind::TaskDue) => "Open Stvdy to see it.".to_string(),
        Some(ReminderKind::EventSoon) => "Open Stvdy to see it.".to_string(),
        Some(ReminderKind::CardsDue) => "Open Stvdy to review.".to_string(),
        None => reminder.subject_key.clone(),
    }
}

/// The instant a reminder's own text was rendered at, for a caller that wants it. Unused by
/// the delivery path itself, which reads the row.
pub fn rendered_at(state: &AppState) -> DateTime<Utc> {
    state.clock.now()
}
