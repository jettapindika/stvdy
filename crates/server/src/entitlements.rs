//! The server's half of the entitlements engine: the snapshot, the one decision path,
//! and the one place an [`EntitlementError`] becomes a response body.
//!
//! `crates/core/src/entitlements/` is pure — a [`UsageSnapshot`] in, a verdict out, no
//! database and no clock. This module is what feeds it and what acts on it:
//!
//!  * [`snapshot`] builds the whole snapshot in **one** statement. Nine-ish scalar
//!    subselects in a single round trip, because the snapshot has to be one point in
//!    time: read one counter per query and a write between two of them produces a
//!    snapshot that never existed. Every count is scoped by `user_id` and by the user's
//!    own local boundary through `chrono-tz` (A6), never a fixed offset.
//!  * [`require`] is the decision every handler makes. A handler writes one line and
//!    nothing else.
//!  * [`reserve_in`] is the monthly-reservation sequence Phase 5 built for decks and
//!    quizzes, defined **once** here so the report and cheatsheet reservations cannot be
//!    a third and fourth copy of it.
//!  * [`refusal`] is the mapping into the `AppError::Entitlement` envelope
//!    `crate::error` already maps to a status and always serialises with all five keys.
//!
//! Nothing here hardcodes a limit or a cost: the matrix and the cost table are
//! `config/plans.toml`, handed to the engine by [`decide`].

use std::future::Future;

use sqlx::{PgConnection, PgPool};
use stvdy_core::config::{Limit, PlanId, PlansFile};
use stvdy_core::entitlements::{
    Credits, EntitlementError, Feature, PlanFacts, SubStatus, UsageSnapshot, check_with,
};
use uuid::Uuid;

use crate::auth::extract::AuthUser;
use crate::credits::Spent;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The caller's plan, as the decision sees it: `users.plan_id` is honoured only while
/// the subscription is active or in grace (A146).
///
/// An expired subscription therefore drops the account to the free limits without
/// anyone editing `users.plan_id`, and a status the code does not understand grants
/// nothing — the safe direction, because a status we cannot read must not lift a limit.
///
/// **A stored `plan_id` with no `subscriptions` row at all is free, by this same rule.**
/// An account whose plan is recorded but whose subscription is not live has no
/// entitlement to paid limits, and a genuinely free account has no row either, so the
/// two are one case and one rule covers both. (`SubStatus::None` is still a distinct
/// value from `Expired`, and `GET /usage` shows the difference; the *decision* does not
/// treat them differently.) A comped account before Phase 13's manual plan switch exists
/// is deliberately not a second rule here: an exception that grants paid limits without
/// a subscription row is exactly the bypass the status check is for.
pub fn effective_plan(plan_id: &str, status: SubStatus) -> PlanId {
    if status.grants_paid_limits() {
        PlanId::parse(plan_id)
    } else {
        PlanId::Free
    }
}

/// The plan-dependent facts the engine needs that are not counters.
///
/// `upgrade_offerable` comes from the **device row**, never the request body (A144):
/// `config/billing_policy.toml` forbids a purchase call to action on iOS and Android, so
/// a locked feature is `FeatureLocked` there and `PlanRequired` everywhere else.
pub fn facts(state: &AppState, user: &AuthUser) -> PlanFacts {
    PlanFacts {
        upgrade_offerable: user.upgrade_offerable(&state.config.billing_policy),
    }
}

/// The whole snapshot for an authenticated caller, in one query.
pub async fn snapshot(state: &AppState, user: &AuthUser) -> AppResult<UsageSnapshot> {
    let mut conn = acquire(state).await?;
    snapshot_in(&mut conn, state, user).await
}

/// The same snapshot from a `user_id` alone, for a caller that has no session: the job
/// worker re-checks an `IngestSource` when it runs, and the device limit is enforced at
/// login before an `AuthUser` exists.
pub async fn snapshot_for(state: &AppState, user_id: Uuid) -> AppResult<UsageSnapshot> {
    let row: Option<(String, String, Option<String>)> = sqlx::query_as(
        "select u.plan_id, u.timezone,
                (select sub.status
                   from subscriptions sub
                  where sub.user_id = u.id
                  order by (sub.status in ('active', 'grace')) desc,
                           sub.current_period_end desc
                  limit 1) as subscription_status
           from users u
          where u.id = $1 and u.deleted_at is null",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the account: {error}")))?;

    let Some((plan_id, timezone, raw_status)) = row else {
        return Err(AppError::NotFound);
    };

    let status = subscription_status(raw_status.as_deref(), user_id);
    let plan = effective_plan(&plan_id, status);
    let mut conn = acquire(state).await?;
    // No device: the caller has no session, so there is no device row to read a flag from.
    // The only features this serves are `DeviceLogin` (checked where the device is written)
    // and a job's own re-check.
    Ok(
        counters(&mut conn, state, user_id, &timezone, plan, status, None)
            .await?
            .snapshot,
    )
}

/// The snapshot read on a caller's own connection, so a count a decision depends on can
/// be read inside the transaction that acts on it (see [`reserve_in`]).
pub async fn snapshot_in(
    conn: &mut PgConnection,
    state: &AppState,
    user: &AuthUser,
) -> AppResult<UsageSnapshot> {
    let plan = user.effective_plan();
    Ok(counters(
        conn,
        state,
        user.user_id,
        &user.timezone,
        plan,
        user.subscription_status,
        Some(user.device_id),
    )
    .await?
    .snapshot)
}

/// The one decision every handler makes.
///
/// Returns `AppError` so a handler writes `entitlements::require(&state, &user,
/// Feature::ChatMessage).await?;` and nothing else. A refusal is logged with the plan and
/// the feature, because it is the only trace an abuse attempt leaves.
///
/// A **read-only device** refuses here, before the feature is even considered: the spec
/// flags a device over the new plan's `devices_max` read-only rather than revoking it, so
/// the device keeps its session and its reads but may not spend — the remedy is to sign
/// another device out, not to buy, which is `FeatureLocked`'s rule (A143). The flag comes
/// from the device row in the same statement as the counters, so the decision is one point
/// in time and one round trip.
pub async fn require(state: &AppState, user: &AuthUser, feature: Feature) -> AppResult<()> {
    let mut conn = acquire(state).await?;
    let counters = counters(
        &mut conn,
        state,
        user.user_id,
        &user.timezone,
        user.effective_plan(),
        user.subscription_status,
        Some(user.device_id),
    )
    .await?;

    if counters.device_read_only {
        tracing::info!(user_id = %user.user_id, device_id = %user.device_id, ?feature, "read-only device refused");
        return Err(device_read_only(Some(feature)));
    }

    decide(state, &counters.snapshot, &facts(state, user), feature)
}

/// [`require`] for a caller that has no `AuthUser` yet.
///
/// One case today: the device limit, enforced where the device row is written (login and
/// signup), which is before a session exists. The snapshot comes from the user id alone,
/// exactly as it does for a job.
pub async fn require_for(state: &AppState, user_id: Uuid, feature: Feature) -> AppResult<()> {
    let snapshot = snapshot_for(state, user_id).await?;
    let facts = PlanFacts {
        // No device row to read a platform from, and the only feature this serves is
        // `DeviceLogin`, whose refusal is a count and never offers a purchase. False is
        // the safe value: it can only produce `FeatureLocked`, which names no plan.
        upgrade_offerable: false,
    };
    decide(state, &snapshot, &facts, feature)
}

/// The monthly-reservation sequence, defined once.
///
/// One transaction: the per-user advisory lock, the decision, `credits::spend_in`, the
/// caller's reserving row, commit. The caller's closure runs **inside** the transaction,
/// which is what makes the count the decision read exact: a row written after the commit
/// is a row the next request's count cannot see, and a monthly cap checked against a
/// count that does not include in-flight work is exceeded by asking twice.
///
/// `feature` is `None` for an action the matrix prices but does not count — a cheatsheet
/// and a report section cost credits and consume no monthly allowance, and the contract's
/// `Feature` enum has no variant for either. `None` still takes the lock and still
/// charges; it just skips the matrix decision, so the ledger is the whole gate.
///
/// The closure receives the `usage_events` id the charge wrote, because the reserving row
/// stores it: the job that finishes the work attaches the provider's tokens and cost to
/// *that* row, and looking up "the newest row for this user" instead would attribute the
/// cost to whichever request happened to finish last. It returns whatever the caller
/// needs back — the reserving row's id, or `()` for an update in place — so a row read
/// back is the row this transaction wrote.
pub async fn reserve_in<'c, F, Fut, R>(
    conn: &'c mut PgConnection,
    state: &AppState,
    user: &AuthUser,
    feature: Option<Feature>,
    action: &str,
    cost: i64,
    reserve: F,
) -> AppResult<(Spent, R)>
where
    F: FnOnce(&'c mut PgConnection, Uuid) -> Fut,
    Fut: Future<Output = AppResult<R>>,
{
    // The lock comes first, before any read the decision depends on: without it two
    // concurrent requests both read the same count and both pass on the last slot. It is
    // transaction-scoped and re-entrant, so `spend_in` taking it again below does not
    // deadlock against this same connection.
    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user.user_id.to_string())
        .execute(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("taking the reserve lock: {error}")))?;

    // The device check is here, and **before** the `feature` branch, because it applies
    // whether or not the action is counted: a cheatsheet and a report section spend credits
    // and consume no monthly allowance, and a read-only device may not spend either of them
    // (the spec flags a device over `devices_max` read-only rather than revoking it). Without
    // this the flag would be enforced on the routes that call `require` and not on the four
    // that reserve — a control with a hole in it.
    let counters = counters(
        &mut *conn,
        state,
        user.user_id,
        &user.timezone,
        user.effective_plan(),
        user.subscription_status,
        Some(user.device_id),
    )
    .await?;

    if counters.device_read_only {
        tracing::info!(user_id = %user.user_id, device_id = %user.device_id, "read-only device refused a reservation");
        return Err(device_read_only(feature));
    }

    if let Some(feature) = feature {
        decide(state, &counters.snapshot, &facts(state, user), feature)?;
    }

    let spent = crate::credits::spend_in(
        &mut *conn,
        state,
        user.user_id,
        user.effective_plan(),
        action,
        cost,
        [None, None],
    )
    .await?;

    let reserved = reserve(conn, spent.usage_event_id).await?;

    Ok((spent, reserved))
}

/// How many rows a read-only recompute left flagged, per table.
///
/// The count of rows that **are** read-only after the call, not of rows it changed: the
/// recompute is idempotent, so a caller can log this on every pass and read it as the
/// account's state rather than as an event.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct ReadOnlyReport {
    pub workspaces: u64,
    pub sources: u64,
    pub devices: u64,
}

/// The three tables a downgrade can put over a limit.
///
/// The statements are literals in one arm per table rather than a `format!`ed query, the
/// same shape `credits::RefundTarget` uses and for the same reason: a table name is
/// exactly the kind of value that should never be interpolated into SQL, and `sqlx`
/// refuses a dynamically built statement so the choice is not left to a reviewer.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ReadOnlyAxis {
    Workspaces,
    Sources,
    Devices,
}

impl ReadOnlyAxis {
    /// The plan's limit for this axis.
    fn limit(self, cfg: &stvdy_core::config::PlanConfig) -> Limit {
        match self {
            Self::Workspaces => cfg.workspaces,
            Self::Sources => cfg.sources_total,
            // Only live devices count: a revoked one is signed out, and counting it would
            // make the remedy the refusal names ("sign another device out") do nothing.
            Self::Devices => Limit::Max(cfg.devices_max),
        }
    }

    /// Ranks the live rows oldest-first, flags the ones past `$2`, and returns how many
    /// are over the limit.
    ///
    /// `target` carries **every** row of the account, with a non-live row ranked `0` so its
    /// flag clears. That makes `read_only = true` mean exactly "this live row is over the
    /// limit": a tombstoned or revoked row left holding a stale `true` would be a claim a
    /// later reader — an admin view, a support query — has no way to tell from the real
    /// thing. The `update` runs inside a CTE (a data-modifying CTE always executes to
    /// completion), and the outer `select` counts the same ranking, so the flags and the
    /// number cannot describe two different states. `read_only <> …` keeps a no-op update
    /// from firing the sync trigger on every pass.
    fn flag_sql(self) -> &'static str {
        match self {
            Self::Workspaces => {
                "with ranked as (
                   select id, row_number() over (order by created_at, id) as rn
                     from workspaces
                    where user_id = $1 and deleted_at is null),
                 target as (
                   select w.id, coalesce(r.rn, 0) as rn
                     from workspaces w
                     left join ranked r on r.id = w.id
                    where w.user_id = $1),
                 flagged as (
                   update workspaces t set read_only = (g.rn > $2)
                     from target g
                    where t.id = g.id and t.read_only <> (g.rn > $2)
                   returning t.id)
                 select count(*)::bigint from ranked where rn > $2"
            }
            Self::Sources => {
                "with ranked as (
                   select id, row_number() over (order by created_at, id) as rn
                     from sources
                    where user_id = $1 and deleted_at is null),
                 target as (
                   select s.id, coalesce(r.rn, 0) as rn
                     from sources s
                     left join ranked r on r.id = s.id
                    where s.user_id = $1),
                 flagged as (
                   update sources t set read_only = (g.rn > $2)
                     from target g
                    where t.id = g.id and t.read_only <> (g.rn > $2)
                   returning t.id)
                 select count(*)::bigint from ranked where rn > $2"
            }
            Self::Devices => {
                "with ranked as (
                   select id, row_number() over (order by created_at, id) as rn
                     from devices
                    where user_id = $1 and revoked_at is null),
                 target as (
                   select d.id, coalesce(r.rn, 0) as rn
                     from devices d
                     left join ranked r on r.id = d.id
                    where d.user_id = $1),
                 flagged as (
                   update devices t set read_only = (g.rn > $2)
                     from target g
                    where t.id = g.id and t.read_only <> (g.rn > $2)
                   returning t.id)
                 select count(*)::bigint from ranked where rn > $2"
            }
        }
    }

    /// Clears every flag on an axis no plan limits, so an upgrade lifts the restriction.
    fn clear_sql(self) -> &'static str {
        match self {
            Self::Workspaces => {
                "update workspaces set read_only = false
                  where user_id = $1 and read_only"
            }
            Self::Sources => {
                "update sources set read_only = false
                  where user_id = $1 and read_only"
            }
            Self::Devices => {
                "update devices set read_only = false
                  where user_id = $1 and read_only"
            }
        }
    }

    fn into_report(self, count: u64, report: &mut ReadOnlyReport) {
        match self {
            Self::Workspaces => report.workspaces = count,
            Self::Sources => report.sources = count,
            Self::Devices => report.devices = count,
        }
    }

    /// The name a log line or an error uses for this axis.
    fn name(self) -> &'static str {
        match self {
            Self::Workspaces => "workspaces",
            Self::Sources => "sources",
            Self::Devices => "devices",
        }
    }
}

/// Recomputes the read-only flags for one account, from its effective plan.
///
/// Idempotent, and called from the rollover job and after a plan change, because the two
/// events that can put an account over a limit are a downgrade and a plan switch. A
/// downgrade never deletes: the over-limit rows keep every column and every byte, they
/// stay readable and exportable (A149), and what they lose is writability — which is what
/// the write paths consult the flag for.
///
/// Ordering is deliberate: rows are ranked oldest-first, so the *oldest* stay writable and
/// the newest are flagged, which is the reading a user expects when they are told to
/// delete something.
pub async fn recompute_read_only(
    pool: &PgPool,
    plans: &PlansFile,
    user_id: Uuid,
) -> AppResult<ReadOnlyReport> {
    let row: Option<(String, Option<String>)> = sqlx::query_as(
        "select u.plan_id,
                (select sub.status
                   from subscriptions sub
                  where sub.user_id = u.id
                  order by (sub.status in ('active', 'grace')) desc,
                           sub.current_period_end desc
                  limit 1) as subscription_status
           from users u
          where u.id = $1 and u.deleted_at is null",
    )
    .bind(user_id)
    .fetch_optional(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the account: {error}")))?;

    let Some((plan_id, raw_status)) = row else {
        return Ok(ReadOnlyReport::default());
    };
    let status = subscription_status(raw_status.as_deref(), user_id);
    let plan = effective_plan(&plan_id, status);
    let cfg = plans.get(plan);

    let mut report = ReadOnlyReport::default();
    for axis in [
        ReadOnlyAxis::Workspaces,
        ReadOnlyAxis::Sources,
        ReadOnlyAxis::Devices,
    ] {
        let count = flag_over_limit(pool, axis, axis.limit(cfg), user_id).await?;
        axis.into_report(count, &mut report);
    }
    Ok(report)
}

/// Flags one axis' over-limit rows read-only, and clears the ones that are back within it.
///
/// Returns how many rows are over the limit, so a caller can log the account's state.
async fn flag_over_limit(
    pool: &PgPool,
    axis: ReadOnlyAxis,
    limit: Limit,
    user_id: Uuid,
) -> AppResult<u64> {
    let Some(max) = limit.max() else {
        // Unlimited: nothing on this axis can be over the limit, so every flag clears —
        // which is what makes an upgrade lift the restriction rather than leave it behind.
        sqlx::query(axis.clear_sql())
            .bind(user_id)
            .execute(pool)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("clearing {} flags: {error}", axis.name()))
            })?;
        return Ok(0);
    };

    let count: i64 = sqlx::query_scalar(axis.flag_sql())
        .bind(user_id)
        .bind(i64::from(max))
        .fetch_one(pool)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!(
                "flagging {} read-only: {error}",
                axis.name()
            ))
        })?;

    Ok(u64::try_from(count).unwrap_or(0))
}

/// The account's storage, in bytes: the originals of its live sources (A157).
///
/// The same sum the `UploadSource` decision reads, from the same place, so the number a
/// user is shown and the number their next upload is checked against cannot differ.
pub async fn storage_bytes(pool: &PgPool, user_id: Uuid) -> AppResult<i64> {
    let bytes: i64 = sqlx::query_scalar(
        "select coalesce(sum(file_size), 0)::bigint from sources
          where user_id = $1 and deleted_at is null",
    )
    .bind(user_id)
    .fetch_one(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("summing storage: {error}")))?;
    Ok(bytes)
}

/// A row a write is addressed to, for the read-only check.
///
/// Two kinds, because those are the two the recompute flags: a workspace (whose refusal
/// reaches the uploads inside it) and a device (whose refusal reaches the spending the
/// device asks for). A source has no write path of its own — it is created into a
/// workspace and deleted, and deleting is the remedy the refusal names.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReadOnlyRow {
    Workspace,
    Device,
}

impl ReadOnlyRow {
    /// What a refusal calls it, and the plan field its limit comes from.
    fn label(self) -> &'static str {
        match self {
            Self::Workspace => "workspace",
            Self::Device => "device",
        }
    }
}

/// Refuses a write into a row the account's plan has put over a limit.
///
/// The spec's downgrade rule is "never delete user data": the over-limit rows keep every
/// column and every byte and stay readable and exportable (A149), and what they lose is
/// writability. This is that refusal, and it is a real one on the write path rather than a
/// UI affordance — a client that skipped the UI would meet it here.
///
/// **Replacing the ownership check, not sitting beside it**, on the write paths that use
/// it: one statement decides both, so a foreign row cannot be read-only *for this caller*
/// and the 404 that hides its existence is the same 404 the route already gives.
///
/// Deleting is deliberately not refused anywhere. It is the remedy both this refusal and
/// the engine's count refusal name, and a read-only row that could not be deleted would
/// leave the account permanently over its limit with nothing it could do.
pub async fn require_writable_row(
    state: &AppState,
    user: &AuthUser,
    row: ReadOnlyRow,
    id: Uuid,
) -> AppResult<()> {
    let plan = user.effective_plan();
    let cfg = state.config.plans.get(plan);

    let found: Option<(bool, i64)> = match row {
        ReadOnlyRow::Workspace => {
            sqlx::query_as(
                "select w.read_only,
                        (select count(*)::bigint from workspaces
                          where user_id = $1 and deleted_at is null) as used
                   from workspaces w
                  where w.id = $2 and w.user_id = $1 and w.deleted_at is null",
            )
            .bind(user.user_id)
            .bind(id)
            .fetch_optional(&state.pool)
            .await
        }
        ReadOnlyRow::Device => {
            sqlx::query_as(
                "select d.read_only,
                        (select count(*)::bigint from devices
                          where user_id = $1 and revoked_at is null) as used
                   from devices d
                  where d.id = $2 and d.user_id = $1 and d.revoked_at is null",
            )
            .bind(user.user_id)
            .bind(id)
            .fetch_optional(&state.pool)
            .await
        }
    }
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!(
            "reading the {} read-only flag: {error}",
            row.label()
        ))
    })?;

    let Some((read_only, used)) = found else {
        return Err(AppError::NotFound);
    };
    if !read_only {
        return Ok(());
    }

    let limit = match row {
        ReadOnlyRow::Workspace => cfg.workspaces,
        ReadOnlyRow::Device => Limit::Max(cfg.devices_max),
    };
    // An unlimited plan cannot have a flagged row (the recompute clears them), so this is
    // the honest "no maximum" rather than a zero that reads as a real cap.
    let limit = limit.max().map_or(i64::MAX, i64::from);

    tracing::info!(user_id = %user.user_id, row = row.label(), %id, plan = %plan, "read-only row refused a write");
    Err(entitlement(
        "limit_reached",
        format!(
            "this {} is read-only after your plan change: your plan allows {limit} and you have {used}",
            row.label()
        ),
        plan_for_more_credits(plan),
        Some(limit),
        Some(used),
    ))
}

/// The refusal for a device over the plan's `devices_max` (A143).
///
/// `FeatureLocked`, not `PlanRequired`: the remedy is to sign another device out, and
/// selling an upgrade to a device that is already over its plan's device count would be
/// offering the wrong fix. `limit`/`used` stay null because this refuses before the
/// feature's own numbers mean anything.
///
/// `None` is the reserve path, where the action is priced but not counted (a cheatsheet, a
/// report section) and there is no `Feature` to name.
fn device_read_only(feature: Option<Feature>) -> AppError {
    entitlement(
        "feature_locked",
        format!(
            "this device is over your plan's device limit, so {} is unavailable here; \
             sign another device out to use it",
            feature.map_or("this action", label)
        ),
        None,
        None,
        None,
    )
}

/// The plan's limits and features, as the plan/usage screen needs them.
///
/// Prices are deliberately absent: the screen names the plan, and this phase shows no
/// price anywhere. Every value is read from the loaded matrix, so a retuned
/// `config/plans.toml` changes this view without a code edit.
pub fn limits_view(plans: &PlansFile, plan: PlanId) -> serde_json::Value {
    let cfg = plans.get(plan);
    serde_json::json!({
        "plan": plan.as_str(),
        "name": cfg.name,
        "workspaces": cfg.workspaces,
        "sources_total": cfg.sources_total,
        "max_file_mb": cfg.max_file_mb,
        "storage_mb": cfg.storage_mb,
        "devices_max": cfg.devices_max,
        "credits_per_month": cfg.credits_per_month,
        "chat_msgs_per_day": cfg.chat_msgs_per_day,
        "web_searches_per_day": cfg.web_searches_per_day,
        "flashcards_per_month": cfg.flashcards_per_month,
        "quizzes_per_month": cfg.quizzes_per_month,
        "reports_per_month": cfg.reports_per_month,
        "syllabus_imports_total": cfg.syllabus_imports_total,
        "google_accounts_max": cfg.google_accounts_max,
        "export_formats": cfg.export_formats,
        "task_ai_breakdown": cfg.task_ai_breakdown,
        "quick_capture": cfg.quick_capture,
        "google_calendar": cfg.google_calendar,
        "planner": cfg.planner,
        "agent_actions": cfg.agent_actions,
        "reminder_channels": cfg.reminder_channels,
        "model_tier": cfg.model_tier,
        "byok": cfg.byok,
        "advanced_analytics": cfg.advanced_analytics,
    })
}

/// The one place an [`EntitlementError`] becomes an `AppError`.
///
/// The envelope is unchanged: `crate::error::AppError::status` already maps
/// `insufficient_credits` to 402, `limit_reached` to 422 and the rest to 403, and
/// `IntoResponse` always writes all five keys with `null` for the absent ones. So this
/// changes no response body's shape.
///
/// `plan` is the caller's **effective** plan. Only `InsufficientCredits` needs it: the
/// variant carries the cost and the balance but not the plan, and a spent-out user has to
/// be told which plan raises the allowance (the chat suite asserts `min_plan: "pro"` for a
/// free account). Every other variant is complete on its own.
pub fn refusal(plan: PlanId, error: EntitlementError) -> AppError {
    match error {
        EntitlementError::PlanRequired { feature, min_plan } => entitlement(
            "plan_required",
            format!("{} is not included in your current plan", label(feature)),
            min_plan,
            None,
            None,
        ),
        EntitlementError::FeatureLocked { feature } => entitlement(
            "feature_locked",
            format!("{} is not available on this device", label(feature)),
            None,
            None,
            None,
        ),
        EntitlementError::LimitReached {
            feature,
            limit,
            used,
            min_plan,
        } => entitlement(
            "limit_reached",
            format!("{}: your plan allows {limit}", label(feature)),
            min_plan,
            Some(limit),
            Some(used),
        ),
        EntitlementError::InsufficientCredits { needed, available } => entitlement(
            "insufficient_credits",
            format!("this action costs {needed} credits and you have {available}"),
            plan_for_more_credits(plan),
            None,
            None,
        ),
    }
}

/// The plan-free form of [`refusal`].
///
/// It cannot name a plan for `InsufficientCredits`, because the error carries none, so
/// the upgrade hint is absent there. Handlers go through [`require`], which passes the
/// caller's plan; this exists for a caller holding an `EntitlementError` and nothing else.
impl From<EntitlementError> for AppError {
    fn from(error: EntitlementError) -> Self {
        match error {
            EntitlementError::InsufficientCredits { needed, available } => entitlement(
                "insufficient_credits",
                format!("this action costs {needed} credits and you have {available}"),
                None,
                None,
                None,
            ),
            other => refusal(PlanId::Free, other),
        }
    }
}

/// Runs the engine and turns a refusal into the response body, logging it.
fn decide(
    state: &AppState,
    snapshot: &UsageSnapshot,
    facts: &PlanFacts,
    feature: Feature,
) -> AppResult<()> {
    match check_with(&state.config.plans, facts, snapshot, feature) {
        Ok(()) => Ok(()),
        Err(error) => {
            let refusal = refusal(snapshot.plan, error);
            tracing::info!(
                plan = %snapshot.plan,
                ?feature,
                code = refusal.code(),
                "entitlement refused"
            );
            Err(refusal)
        }
    }
}

/// Builds the `AppError::Entitlement` body. One constructor, so no refusal can be missing
/// a key or carry a field in a different shape than the client's mapper expects.
fn entitlement(
    code: &'static str,
    message: String,
    min_plan: Option<PlanId>,
    limit: Option<i64>,
    used: Option<i64>,
) -> AppError {
    AppError::Entitlement {
        code,
        message,
        min_plan: min_plan.map(PlanId::as_str),
        limit,
        used,
    }
}

/// The plan that would raise the credit allowance: the next one up the ladder.
///
/// Derived from `PlanId::ALL` rather than written down, the same rule
/// `crates/core/src/entitlements` applies to a countable limit's `min_plan`, so a plan
/// added later joins the ladder without a second place to edit. `None` on the top plan
/// means "not for sale", which is the truth.
fn plan_for_more_credits(plan: PlanId) -> Option<PlanId> {
    PlanId::ALL
        .iter()
        .skip_while(|candidate| **candidate != plan)
        .nth(1)
        .copied()
}

/// What a refusal calls the feature it refused. Only the message uses this; the machine
/// readable fields are the code and `min_plan`.
fn label(feature: Feature) -> &'static str {
    match feature {
        Feature::CreateWorkspace => "a workspace",
        Feature::UploadSource { .. } => "this upload",
        Feature::IngestSource => "a source",
        Feature::ChatMessage => "chat messages today",
        Feature::WebSearch => "web searches today",
        Feature::CreateDeck => "a deck",
        Feature::FlashcardBatch { .. } => "flashcards this month",
        Feature::CreateQuiz { .. } => "quizzes this month",
        Feature::Report => "reports this month",
        Feature::ExportFormat(_) => "this export format",
        Feature::TaskBreakdown => "task breakdown",
        Feature::QuickCapture => "quick capture",
        Feature::GoogleCalendar => "Google Calendar",
        Feature::SyllabusImport => "a syllabus import",
        Feature::PlanGeneration => "automatic planning",
        Feature::AgentAction => "agent actions",
        Feature::DeviceLogin => "devices",
        Feature::Byok => "your own model key",
        Feature::ReminderChannel(_) => "this reminder channel",
        Feature::AdvancedAnalytics => "advanced analytics",
    }
}

/// The subscription status from the user's live row, logging one it cannot read.
///
/// `None` is an account with no subscription row at all (free). An unrecognised string is
/// treated the same way and logged: a status this code does not understand must not grant
/// paid limits (A146), and the log line is how the gap becomes visible.
pub(crate) fn subscription_status(raw: Option<&str>, user_id: Uuid) -> SubStatus {
    let Some(raw) = raw else {
        return SubStatus::None;
    };
    let parsed = SubStatus::parse(raw);
    if parsed == SubStatus::None {
        tracing::warn!(%user_id, status = %raw, "unrecognised subscription status; treating the account as free");
    }
    parsed
}

async fn acquire(state: &AppState) -> AppResult<sqlx::pool::PoolConnection<sqlx::Postgres>> {
    state
        .pool
        .acquire()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("acquiring a connection: {error}")))
}

/// The counters themselves, on a connection the caller owns.
///
/// **One statement**, so the snapshot is a single point in time. Every count is scoped by
/// `user_id` and by the user's own local boundary: `day_start` and `month_start` come from
/// `chrono-tz` through `crate::util`, so a user in a zone that is not UTC sees the reset
/// when their own calendar says so.
///
/// The flashcard count is the one that is not a plain `count(*)`: it adds the cards
/// in-flight decks have **reserved**, because a deck that is still generating has no
/// `flashcard` rows yet and a count that ignored it would read zero and let a burst of
/// requests each pass on the last slot. It is the same expression
/// `flashcards/routes.rs` used inline before this module existed.
async fn counters(
    conn: &mut PgConnection,
    state: &AppState,
    user_id: Uuid,
    timezone: &str,
    plan: PlanId,
    subscription_status: SubStatus,
    device_id: Option<Uuid>,
) -> AppResult<CallerCounters> {
    let day_start = crate::util::local_day_start(timezone, state.clock.as_ref());
    let month_start = crate::util::local_month_start(timezone, state.clock.as_ref());

    let row = sqlx::query_as::<_, SnapshotRow>(
        "select
           (select coalesce(sum(delta) filter (where bucket = 'monthly'), 0)::bigint
              from credit_ledger where user_id = $1) as credits_monthly,
           (select coalesce(sum(delta) filter (where bucket = 'purchased'), 0)::bigint
              from credit_ledger where user_id = $1) as credits_purchased,
           (select coalesce(sum(file_size), 0)::bigint
              from sources where user_id = $1 and deleted_at is null) as storage_bytes,
           (select count(*)::bigint
              from workspaces where user_id = $1 and deleted_at is null) as workspaces,
           (select count(*)::bigint
              from sources where user_id = $1 and deleted_at is null) as sources,
           (select count(*)::bigint
              from devices where user_id = $1 and revoked_at is null) as devices,
           (select count(*)::bigint
              from usage_events
             where user_id = $1 and action = 'chat_message' and created_at >= $2) as chat_msgs_today,
           (select count(*)::bigint
              from usage_events
             where user_id = $1 and action = 'web_search' and cache_hit = false
               and created_at >= $2) as web_searches_today,
           (select (select count(*)::bigint from flashcards
                      where user_id = $1 and deleted_at is null and created_at >= $3)
                 + (select coalesce(sum(requested_count), 0)::bigint from decks
                      where user_id = $1 and deleted_at is null
                        and status = 'generating' and created_at >= $3)) as flashcards_this_month,
           (select count(*)::bigint from quizzes
             where user_id = $1 and deleted_at is null and created_at >= $3
               and status in ('ready', 'generating')) as quizzes_this_month,
           (select count(*)::bigint from reports
             where user_id = $1 and deleted_at is null and created_at >= $3
               and status in ('draft', 'generating', 'ready')) as reports_this_month,
           (select count(*)::bigint from sources
             where user_id = $1 and deleted_at is null and kind = 'syllabus') as syllabus_imports,
           -- The one fact about the calling *device* rather than the account: the spec
           -- flags a device over the plan's `devices_max` read-only instead of revoking it.
           -- The coalesce is *outside* the subselect: with no matching row the subselect is
           -- null, and `coalesce` inside it would only cover a null column. A null `$4` (a
           -- caller with no device row, e.g. a job) therefore reads false.
           coalesce((select read_only from devices where id = $4), false) as device_read_only",
    )
    .bind(user_id)
    .bind(day_start)
    .bind(month_start)
    .bind(device_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the usage snapshot: {error}")))?;

    Ok(CallerCounters {
        snapshot: UsageSnapshot {
            plan,
            subscription_status,
            credits: Credits {
                monthly: row.credits_monthly,
                purchased: row.credits_purchased,
            },
            storage_bytes: row.storage_bytes,
            workspaces: row.workspaces,
            sources: row.sources,
            devices: row.devices,
            chat_msgs_today: row.chat_msgs_today,
            web_searches_today: row.web_searches_today,
            flashcards_this_month: row.flashcards_this_month,
            quizzes_this_month: row.quizzes_this_month,
            reports_this_month: row.reports_this_month,
            syllabus_imports: row.syllabus_imports,
        },
        device_read_only: row.device_read_only,
    })
}

/// The counters, plus the one fact that is about the calling device rather than the
/// account.
///
/// Returned together because they are read in one statement: the read-only flag has to
/// come from the same instant as the counters it is decided beside, or a plan change
/// between two queries would produce a decision that never existed.
struct CallerCounters {
    snapshot: UsageSnapshot,
    /// The calling device is over the plan's device limit: it may read, not spend.
    device_read_only: bool,
}

/// The snapshot's wire shape, so the twelve subselects decode in one pass.
#[derive(sqlx::FromRow)]
struct SnapshotRow {
    credits_monthly: i64,
    credits_purchased: i64,
    storage_bytes: i64,
    workspaces: i64,
    sources: i64,
    devices: i64,
    chat_msgs_today: i64,
    web_searches_today: i64,
    flashcards_this_month: i64,
    quizzes_this_month: i64,
    reports_this_month: i64,
    syllabus_imports: i64,
    device_read_only: bool,
}

#[cfg(test)]
mod tests {
    use super::*;
    use stvdy_core::config::load_plans;

    fn plans() -> PlansFile {
        let root = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..");
        load_plans(&root.join("config/plans.toml")).expect("plans.toml loads")
    }

    /// The mapping the response bodies depend on, asserted against the roadmap's § 5
    /// table: `plan_required`/`feature_locked` 403, `limit_reached` 422,
    /// `insufficient_credits` 402.
    #[test]
    fn the_entitlement_errors_map_to_the_roadmaps_status_and_code() {
        let plan_required = AppError::from(EntitlementError::PlanRequired {
            feature: Feature::ExportFormat(stvdy_core::config::ExportFormat::Pdf),
            min_plan: Some(PlanId::Pro),
        });
        assert_eq!(plan_required.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(plan_required.code(), "plan_required");

        let locked = AppError::from(EntitlementError::FeatureLocked {
            feature: Feature::Byok,
        });
        assert_eq!(locked.status(), axum::http::StatusCode::FORBIDDEN);
        assert_eq!(locked.code(), "feature_locked");

        let reached = AppError::from(EntitlementError::LimitReached {
            feature: Feature::CreateWorkspace,
            limit: 2,
            used: 2,
            min_plan: Some(PlanId::Pro),
        });
        assert_eq!(
            reached.status(),
            axum::http::StatusCode::UNPROCESSABLE_ENTITY
        );
        assert_eq!(reached.code(), "limit_reached");

        let short = AppError::from(EntitlementError::InsufficientCredits {
            needed: 6,
            available: 0,
        });
        assert_eq!(short.status(), axum::http::StatusCode::PAYMENT_REQUIRED);
        assert_eq!(short.code(), "insufficient_credits");
    }

    /// The refusal a spent-out free user gets names the plan that raises the allowance:
    /// `From` cannot (the error carries no plan), `refusal` can.
    #[test]
    fn a_short_balance_names_the_next_plan_up() {
        let error = EntitlementError::InsufficientCredits {
            needed: 1,
            available: 0,
        };
        match refusal(PlanId::Free, error) {
            AppError::Entitlement {
                code,
                min_plan,
                limit,
                used,
                message,
            } => {
                assert_eq!(code, "insufficient_credits");
                assert_eq!(min_plan, Some("pro"));
                assert_eq!(limit, None);
                assert_eq!(used, None);
                assert!(message.contains("1 credit"), "{message}");
            }
            other => panic!("expected an entitlement error, got {other:?}"),
        }

        // The top plan has nothing to sell, so it must not hint at one.
        match refusal(PlanId::Power, error) {
            AppError::Entitlement { min_plan, .. } => assert_eq!(min_plan, None),
            other => panic!("expected an entitlement error, got {other:?}"),
        }
    }

    /// A limit refusal reports the numbers the UI renders as "3 of 3", from the engine's
    /// own figures rather than recomputed here.
    #[test]
    fn a_limit_refusal_carries_the_numbers_and_the_upgrade() {
        match refusal(
            PlanId::Free,
            EntitlementError::LimitReached {
                feature: Feature::CreateWorkspace,
                limit: 2,
                used: 2,
                min_plan: Some(PlanId::Pro),
            },
        ) {
            AppError::Entitlement {
                code,
                min_plan,
                limit,
                used,
                ..
            } => {
                assert_eq!(code, "limit_reached");
                assert_eq!(min_plan, Some("pro"));
                assert_eq!(limit, Some(2));
                assert_eq!(used, Some(2));
            }
            other => panic!("expected an entitlement error, got {other:?}"),
        }
    }

    /// The effective plan is `users.plan_id` only while the subscription is live (A146),
    /// and a status the code cannot read grants nothing.
    #[test]
    fn an_inactive_subscription_drops_the_stored_plan() {
        assert_eq!(effective_plan("pro", SubStatus::Active), PlanId::Pro);
        assert_eq!(effective_plan("pro", SubStatus::Grace), PlanId::Pro);
        assert_eq!(effective_plan("pro", SubStatus::Expired), PlanId::Free);
        assert_eq!(effective_plan("pro", SubStatus::Canceled), PlanId::Free);
        assert_eq!(effective_plan("pro", SubStatus::None), PlanId::Free);
        // An unrecognised stored plan is free too: a corrupt value must not grant more.
        assert_eq!(
            effective_plan("enterprise", SubStatus::Active),
            PlanId::Free
        );
    }

    /// The view the plan screen renders comes from the matrix, not from a literal here.
    #[test]
    fn the_limits_view_is_the_matrixs_own_numbers() {
        let plans = plans();
        let view = limits_view(&plans, PlanId::Free);
        let cfg = plans.get(PlanId::Free);

        assert_eq!(view["plan"], "free");
        assert_eq!(view["name"], cfg.name);
        assert_eq!(view["workspaces"], serde_json::json!(cfg.workspaces));
        assert_eq!(view["sources_total"], serde_json::json!(cfg.sources_total));
        assert_eq!(view["max_file_mb"], serde_json::json!(cfg.max_file_mb));
        assert_eq!(view["credits_per_month"], cfg.credits_per_month);
        assert_eq!(view["export_formats"], serde_json::json!(["md"]));
        assert_eq!(view["byok"], false);
        // Prices are the admin's business, not this view's.
        assert!(view.get("price_idr_monthly").is_none(), "{view}");

        // An unlimited limit serialises the way the config's own `Limit` does, so the
        // client reads one shape.
        let pro = limits_view(&plans, PlanId::Pro);
        assert_eq!(pro["workspaces"], "unlimited");
        assert_eq!(pro["sources_total"], 150);
    }

    /// The upgrade hint is the next plan up the ladder, derived from `PlanId::ALL`.
    #[test]
    fn the_credit_upgrade_hint_follows_the_ladder() {
        assert_eq!(plan_for_more_credits(PlanId::Free), Some(PlanId::Pro));
        assert_eq!(plan_for_more_credits(PlanId::Pro), Some(PlanId::Power));
        assert_eq!(plan_for_more_credits(PlanId::Power), None);
    }
}
