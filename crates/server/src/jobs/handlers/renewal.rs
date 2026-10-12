//! `renewal_reminder`: the 7/3/1/0-day notice before a paid period ends (A300).
//!
//! **A producer, not a second delivery path.** This module inserts a `reminders` row and
//! enqueues Phase 12's `send_push` for it, exactly as the reminder engine does for a task
//! or a set of due cards. Phase 12 owns the channels, the quiet hours and the delivery
//! rows; what this module owns is *when* a renewal reminder exists and *what it may say*.
//!
//! Two rules, and each is the kind of thing that goes wrong silently:
//!
//!  * **The period is the watermark, and the boundary is the key.** `dedupe_key =
//!    "renewal_reminder:<user_id>:<period_end date>:<days>"` (A300) means a second
//!    scheduler pass in the same period enqueues nothing — the key *is* the record that
//!    this boundary was handled, exactly as the grant's period key is (A152). It also
//!    means a **renewal moves the boundary and orphans the old reminders for free**: the
//!    new `current_period_end` produces new keys, and the job for an old boundary re-reads
//!    the period and finds it has moved.
//!  * **A reminder that fires at 03:00 is a reminder nobody reads**, so `run_at` is the
//!    account's own local 09:00 — through `chrono-tz`, never a fixed offset (A6). And a
//!    boundary whose 09:00 has already passed is **not enqueued at all**: a "seven days
//!    before" notice that arrives five days late is not a reminder, it is noise.
//!
//! The copy follows `billing_policy` (A300). The rule is the conservative one, because a
//! push is addressed to an *account* whose devices the server does not choose between: if
//! **any** live device is on a platform the policy forbids a purchase call to action on,
//! the copy carries none. A CTA that reaches an iPhone is an App Store violation whether or
//! not a desktop would have tolerated it.

use chrono::{DateTime, Duration as ChronoDuration, NaiveDate, TimeZone, Utc};
use chrono_tz::Tz;
use uuid::Uuid;

use crate::error::{AppError, AppResult};
use crate::jobs::{ClaimedJob, Failure};
use crate::state::AppState;

/// How many days before `current_period_end` a reminder fires. Four boundaries (A300).
pub const BOUNDARY_DAYS: [i64; 4] = [7, 3, 1, 0];

/// The hour, in the account's own zone, at which a reminder is delivered.
pub const REMINDER_HOUR: u32 = 9;

/// The i18n key the client renders the in-app copy from, in both locales.
///
/// The server sends this beside the rendered text because the push body is one English
/// sentence — the same convention `auth::email_verify` follows — while the SPA's reminder
/// feed renders from the key and so gets `en.json` and `id.json` from one source. A key
/// rather than a sentence is also what makes "the same reminder in Indonesian" a locale
/// change rather than a server change.
pub const PLAN_ENDING_KEY: &str = "reminder.planEnding";

/// Where a desktop copy points, and what the copy offers. One constant, because the policy
/// check and the copy must agree about what "a CTA" is.
const PRICING_PATH: &str = "/pricing";

/// The account's live subscription, as the producer needs it.
#[derive(Debug, sqlx::FromRow)]
struct RenewingSubscription {
    subscription_id: Uuid,
    user_id: Uuid,
    plan_id: String,
    current_period_end: DateTime<Utc>,
    timezone: String,
}

/// Enqueues the four boundary jobs for every live subscription whose window is ahead.
///
/// Returns how many subscriptions it examined. Bounded by [`SCHEDULE_BATCH`], like the
/// account pass: a pass over every subscription in the tree would hold a long read open
/// for no gain, and the pass is idempotent so the rest are covered by the next tick.
///
/// [`SCHEDULE_BATCH`]: super::super::schedule::SCHEDULE_BATCH
pub async fn renewal_pass(state: &AppState) -> AppResult<u64> {
    let rows: Vec<RenewingSubscription> = sqlx::query_as(
        "select s.id as subscription_id, s.user_id, s.plan_id,
                s.current_period_end, u.timezone
           from subscriptions s
           join users u on u.id = s.user_id
          where s.status in ('active', 'grace')
            and u.deleted_at is null
            and s.current_period_end > now()
          order by s.current_period_end
          limit $1",
    )
    .bind(crate::jobs::schedule::SCHEDULE_BATCH)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("listing subscriptions to remind: {error}"))
    })?;

    let mut scheduled = 0u64;
    let now = state.clock.now();
    for row in rows {
        for days in BOUNDARY_DAYS {
            let fire_at = boundary_instant(&row.timezone, row.current_period_end, days);
            // A boundary whose 09:00 has already passed is skipped rather than enqueued
            // with a past `run_at`: the worker would deliver it immediately, and "seven
            // days before" arriving after the fact is a notification about nothing.
            if fire_at <= now {
                continue;
            }
            let key = dedupe_key(row.user_id, row.current_period_end, days);
            crate::jobs::enqueue_at(
                &state.pool,
                &state.jobs,
                "renewal_reminder",
                serde_json::json!({
                    "user_id": row.user_id,
                    "subscription_id": row.subscription_id,
                    "plan_id": row.plan_id,
                    "period_end": row.current_period_end.to_rfc3339(),
                    "days": days,
                }),
                Some(&key),
                fire_at,
            )
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("enqueuing a renewal reminder: {error}"))
            })?;
            scheduled += 1;
        }
    }

    if scheduled > 0 {
        tracing::info!(scheduled, "scheduled renewal reminders");
    }
    Ok(scheduled)
}

/// The boundary key: the account, the period's end **date**, and the days before it.
///
/// The end date rather than the instant: the period end is an absolute instant, and two
/// renewals that land a second apart are the same period as far as a reminder is
/// concerned. The date is the account's own local date, so a boundary named "1 day before"
/// is the day before *the student's* end date and not a UTC one.
pub fn dedupe_key(user_id: Uuid, period_end: DateTime<Utc>, days: i64) -> String {
    format!(
        "renewal_reminder:{user_id}:{}:{days}",
        period_end.date_naive()
    )
}

/// The instant `days` before `period_end`, at [`REMINDER_HOUR`] in the account's own zone.
///
/// Local-date arithmetic through `chrono-tz` and never a fixed offset (A6): the day is the
/// *student's* day, and subtracting from the UTC instant would land on the wrong date for
/// every zone east of Greenwich.
pub fn boundary_instant(timezone: &str, period_end: DateTime<Utc>, days: i64) -> DateTime<Utc> {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let end_date = period_end.with_timezone(&tz).date_naive();
    local_hour(&tz, end_date - ChronoDuration::days(days), REMINDER_HOUR)
}

/// [`REMINDER_HOUR`]`:`00 local on `date`, as an absolute instant.
///
/// Built from `NaiveTime::MIN` plus an offset rather than `and_hms_opt`, which is a
/// `Result`-shaped call for a value that cannot fail here.
///
/// An **ambiguous** local hour takes the earlier offset, the convention Phase 10's calendar
/// import already uses for a repeated wall time. A **nonexistent** one — a DST jump over
/// 09:00, which no zone this product serves has, since the jumps land between 00:00 and
/// 03:00 — falls back to reading the wall time as UTC: a `fire_at` has to be *a* value, and
/// this branch is unreachable for a real zone.
fn local_hour(tz: &Tz, date: NaiveDate, hour: u32) -> DateTime<Utc> {
    let naive = date.and_time(chrono::NaiveTime::MIN) + ChronoDuration::hours(i64::from(hour));
    tz.from_local_datetime(&naive)
        .earliest()
        .map(|instant| instant.with_timezone(&Utc))
        .unwrap_or_else(|| naive.and_utc())
}

/// `renewal_reminder`: one boundary, delivered through Phase 12's path.
pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let user_id = job.user_id()?;
    let days = job
        .payload
        .get("days")
        .and_then(|value| value.as_i64())
        .ok_or_else(|| Failure::permanent("renewal_reminder payload has no days"))?;
    let scheduled_end = job
        .payload
        .get("period_end")
        .and_then(|value| value.as_str())
        .and_then(|raw| DateTime::parse_from_rfc3339(raw).ok())
        .map(|instant| instant.with_timezone(&Utc))
        .ok_or_else(|| Failure::permanent("renewal_reminder payload has no period_end"))?;

    let current: Option<(Uuid, DateTime<Utc>)> = sqlx::query_as(
        "select id, current_period_end
           from subscriptions
          where user_id = $1 and status in ('active', 'grace')
          order by current_period_end desc
          limit 1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("reading the subscription: {error}")))?;

    let Some((subscription_id, current_end)) = current else {
        // The subscription lapsed or was removed between the enqueue and the claim. There
        // is nothing to remind anyone about, and no retry would change that.
        tracing::debug!(%user_id, days, "renewal reminder dropped: no live subscription");
        return Ok(());
    };

    // **The renewal case.** A period that moved since this job was enqueued means the
    // student has already renewed: the reminder is about a period that no longer exists,
    // and sending it would tell them their plan is ending when it is not.
    if current_end != scheduled_end {
        tracing::info!(
            %user_id,
            days,
            scheduled_end = %scheduled_end,
            current_end = %current_end,
            "renewal reminder dropped: the period moved"
        );
        return Ok(());
    }

    let plan_and_zone: Option<(String, String)> = sqlx::query_as(
        "select s.plan_id, u.timezone
           from subscriptions s
           join users u on u.id = s.user_id
          where s.id = $1",
    )
    .bind(subscription_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("reading the plan: {error}")))?;

    let (plan, timezone) = plan_and_zone.unwrap_or_else(|| ("free".to_string(), "UTC".to_string()));

    // The boundary this job is for, recomputed from the period end rather than taken from
    // the clock: the row's `local_date` is the *student's* day (the dedupe axis Phase 12's
    // engine keys on), and reading it from the worker's instant would name the wrong day for
    // every account east of Greenwich.
    let fire_at = boundary_instant(&timezone, current_end, days);
    let local_date = fire_at
        .with_timezone(&timezone.parse::<Tz>().unwrap_or(chrono_tz::UTC))
        .date_naive();

    let may_offer_purchase = may_offer_a_purchase(state, user_id)
        .await
        .map_err(|error| Failure::transient(format!("reading the devices: {error}")))?;

    let (title, body) = copy(&plan, current_end, days, may_offer_purchase);

    // The row is the claim unit for the delivery (Phase 12's `send_push` marks it `sent`)
    // and the in-app feed's source, so the text the student sees in the app and the text
    // that arrives as a push are the same sentence from the same row.
    let reminder_id: Option<Uuid> = sqlx::query_scalar(
        "insert into reminders
            (user_id, kind, subject_id, subject_key, local_date, fire_at, dedupe_key,
             status, origin)
         values ($1, 'renewal', null, $2, $3, $4, $5, 'pending', 'server')
         on conflict (dedupe_key) do nothing
         returning id",
    )
    .bind(user_id)
    .bind(subscription_id.to_string())
    .bind(local_date)
    .bind(fire_at)
    .bind(dedupe_key(user_id, current_end, days))
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("writing the reminder: {error}")))?;

    let Some(reminder_id) = reminder_id else {
        // The key is the watermark: this boundary already has its reminder, so a second
        // delivery would be a second notification for one event (A285/A300).
        tracing::debug!(%user_id, days, "renewal reminder already recorded");
        return Ok(());
    };

    crate::jobs::enqueue(
        &state.pool,
        &state.jobs,
        "send_push",
        serde_json::json!({
            "reminder_id": reminder_id,
            "title": title,
            "body": body,
            // Beside the rendered sentence, so the SPA's feed renders the same reminder in
            // either locale from one key rather than re-deriving the copy from the numbers.
            "key": PLAN_ENDING_KEY,
            "plan_id": plan,
            "date": current_end.to_rfc3339(),
            "days": days,
        }),
        Some(&format!("send_push:{reminder_id}")),
    )
    .await
    .map_err(|error| Failure::transient(format!("enqueuing the delivery: {error}")))?;

    tracing::info!(%user_id, days, %reminder_id, may_offer_purchase, "renewal reminder enqueued");
    Ok(())
}

/// Whether the copy may offer a purchase, which is true only when **every** live device of
/// the account is on a platform whose policy allows a purchase call to action.
///
/// The platform comes from `devices.platform` (A144), never from a payload: a push is
/// addressed to the account and the server does not choose which device receives it, so one
/// iPhone in the account is enough to forbid the CTA everywhere. Deny-by-default, like
/// `AuthUser::upgrade_offerable`: a platform the policy cannot classify must not sell.
async fn may_offer_a_purchase(state: &AppState, user_id: Uuid) -> AppResult<bool> {
    let platforms: Vec<String> = sqlx::query_scalar(
        "select platform from devices where user_id = $1 and revoked_at is null",
    )
    .bind(user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the devices: {error}")))?;

    Ok(platforms.iter().all(|platform| {
        stvdy_core::config::Platform::ALL
            .into_iter()
            .find(|known| known.as_str() == platform)
            .is_some_and(|known| {
                state
                    .config
                    .billing_policy
                    .for_platform(known)
                    .allow_purchase_cta_in_locked_messages
            })
    }))
}

/// The reminder's title and body.
///
/// `may_offer_purchase` is the whole difference between the two copies, and the difference
/// is the point: on a platform whose policy forbids it, the sentence names the date and
/// **nothing to buy** — no price, no link, no call to action (A300). The desktop copy is the
/// one that carries the link, which is what makes the policy check observable rather than
/// decorative.
fn copy(
    plan: &str,
    period_end: DateTime<Utc>,
    days: i64,
    may_offer_purchase: bool,
) -> (String, String) {
    let plan = plan_name(plan);
    let date = period_end.format("%-d %B %Y").to_string();

    let when = match days {
        0 => "ends today".to_string(),
        1 => "ends tomorrow".to_string(),
        other => format!("ends in {other} days"),
    };

    let title = format!("Your {plan} plan {when}");
    let body = if may_offer_purchase {
        format!(
            "Your {plan} plan ends on {date}. Renew at {PRICING_PATH} to keep it, or switch \
             to the free plan and keep everything you have made."
        )
    } else {
        format!(
            "Your plan ends on {date}. You can renew on the web at any time; everything you \
             have made stays available."
        )
    };

    (title, body)
}

/// The plan's display name, capitalised, for a sentence a student reads.
fn plan_name(plan: &str) -> String {
    let mut chars = plan.chars();
    match chars.next() {
        Some(first) => first.to_uppercase().collect::<String>() + chars.as_str(),
        None => "Paid".to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn end() -> DateTime<Utc> {
        "2026-11-01T17:00:00Z".parse().expect("a valid instant")
    }

    #[test]
    fn the_boundary_is_the_students_own_nine_am() {
        // 2026-11-01T17:00Z is 2026-11-02T00:00 in Jakarta (UTC+7), so the local end date
        // is the 2nd and "seven days before" is 2026-10-26 at 09:00+07 = 02:00Z. Computing
        // the date in UTC would name the 1st and fire a day early.
        let instant = boundary_instant("Asia/Jakarta", end(), 7);
        assert_eq!(instant.to_rfc3339(), "2026-10-26T02:00:00+00:00");

        let today = boundary_instant("Asia/Jakarta", end(), 0);
        assert_eq!(today.to_rfc3339(), "2026-11-02T02:00:00+00:00");
    }

    #[test]
    fn a_dst_transition_day_uses_the_offset_in_force_then() {
        // `end()` is 2026-11-01T17:00Z, which is 18:00+01:00 in Berlin (CET: the change has
        // already happened), so the local end date is the 1st and seven days before is the
        // 25th. That date *is* the transition day - Berlin goes 03:00 CEST -> 02:00 CET on
        // 2026-10-25 - so 09:00 that morning is already CET: 09:00+01:00 = 08:00Z. The 24th
        // would have been +02:00 (07:00Z), so this case still exercises the offset in force
        // *after* the change rather than the one before it.
        let instant = boundary_instant("Europe/Berlin", end(), 7);
        assert_eq!(instant.to_rfc3339(), "2026-10-25T08:00:00+00:00");
    }

    #[test]
    fn the_key_names_the_period_and_the_boundary() {
        let user_id = Uuid::nil();
        assert_eq!(
            dedupe_key(user_id, end(), 7),
            "renewal_reminder:00000000-0000-0000-0000-000000000000:2026-11-01:7"
        );
        // A renewal that moves the end produces a different key, so the old boundary's
        // reminders are orphaned rather than re-sent.
        assert_ne!(
            dedupe_key(user_id, end(), 7),
            dedupe_key(user_id, end() + ChronoDuration::days(30), 7)
        );
    }

    #[test]
    fn the_mobile_copy_offers_nothing_to_buy() {
        let (_, desktop) = copy("pro", end(), 3, true);
        let (_, mobile) = copy("pro", end(), 3, false);

        assert!(desktop.contains(PRICING_PATH), "{desktop}");
        assert!(!mobile.contains(PRICING_PATH), "{mobile}");
        assert!(!mobile.contains("Renew"), "{mobile}");
        // And the date is in both: the reminder still says what it is for.
        assert!(mobile.contains("1 November 2026"), "{mobile}");
    }
}
