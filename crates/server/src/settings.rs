//! `GET /settings` and `PUT /settings`.
//!
//! The row is **created on first read** ([`load_or_create`]), so a read never has to answer
//! "no settings yet" and a write never has to decide whether to insert. A screen that has
//! one shape is a screen whose loading and empty states cannot disagree.
//!
//! The timezone is deliberately **not** on this row (A203): `users.timezone` predates any
//! settings row, the reminder and planner maths read it for an account that has never opened
//! this screen, and a second copy here is exactly the drift the "local-date maths only
//! through `chrono-tz`" rule would not catch. `GET /settings` therefore returns it beside the
//! row rather than inside it, and `PUT /settings` writes it to `users`.
//!
//! `PUT` is **partial**: the screen saves per section, and a full replace would let a stale
//! tab clobber a channel set changed on another device (A201). What it writes is the whole
//! row, because the roadmap gives `user_settings` record-level LWW and there is no per-field
//! timestamp to merge with.

use axum::Json as AxumJson;
use axum::Router;
use axum::extract::State;
use axum::routing::get;
use chrono::{DateTime, Utc};
use serde_json::{Map, Value, json};
use sqlx::types::Json;
use sqlx::{PgConnection, PgPool};
use uuid::Uuid;

use stvdy_core::config::ReminderChannel;
use stvdy_core::entitlements::Feature;
use stvdy_sync::proto::Entity;

use crate::auth::extract::AuthUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::tasks::Reject;

/// The columns every read and every returning write selects, in one place so a column added
/// to the table and forgotten here is a visible edit rather than a silently missing field.
const SETTINGS_COLUMNS: &str = "study_start_hour, study_end_hour, max_study_min_per_day, \
                                block_len_min, break_min, reminder_channels, \
                                quiet_start_hour, quiet_end_hour, language, theme, updated_at";

/// The `language` values `user_settings.language` accepts.
const LANGUAGES: [&str; 2] = ["en", "id"];

/// The `theme` values `user_settings.theme` accepts.
const THEMES: [&str; 3] = ["system", "light", "dark"];

/// The settings routes, plus the three BYOK ones.
///
/// `/settings/api-key` is a settings path and belongs in this table (Phase 13's plan puts it
/// here rather than in `billing/routes.rs`), while the encryption and the provider seam live
/// in `billing/byok.rs`: the path is where the screen looks, the rule is where the key is.
pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/settings", get(read).put(write))
        .route(
            "/settings/api-key",
            get(api_key_read).put(api_key_write).delete(api_key_delete),
        )
}

/// `GET /settings/api-key`: whether a key is set, and where it points. **Never the key.**
pub async fn api_key_read(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<AxumJson<crate::billing::byok::ByokDescription>> {
    Ok(AxumJson(
        crate::billing::byok::describe(&state, &user).await?,
    ))
}

/// `PUT /settings/api-key`: store or replace the account's key, encrypted at rest.
///
/// The plan gate and the platform refusal are inside `byok::set`, so a caller cannot reach the
/// encrypt without passing both.
pub async fn api_key_write(
    State(state): State<AppState>,
    user: AuthUser,
    AxumJson(body): AxumJson<crate::billing::byok::ByokRequest>,
) -> AppResult<AxumJson<crate::billing::byok::ByokDescription>> {
    Ok(AxumJson(
        crate::billing::byok::set(&state, &user, body).await?,
    ))
}

/// `DELETE /settings/api-key`: remove it. Idempotent, so a double-click is safe.
pub async fn api_key_delete(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<AxumJson<crate::billing::byok::ByokDescription>> {
    Ok(AxumJson(crate::billing::byok::clear(&state, &user).await?))
}

/// The account's settings row.
///
/// `reminder_channels` is `Json<Vec<..>>` rather than a `Vec` because the column is `jsonb`
/// and the enum is the config's own (A142): a channel the plan matrix does not know about
/// must not be representable in a stored row.
#[derive(Debug, Clone, PartialEq, sqlx::FromRow, serde::Serialize)]
pub struct SettingsRow {
    pub study_start_hour: i32,
    pub study_end_hour: i32,
    pub max_study_min_per_day: i32,
    pub block_len_min: i32,
    pub break_min: i32,
    pub reminder_channels: Json<Vec<ReminderChannel>>,
    pub quiet_start_hour: Option<i32>,
    pub quiet_end_hour: Option<i32>,
    pub language: String,
    pub theme: String,
    pub updated_at: DateTime<Utc>,
}

/// The caller's settings row, created with the column defaults if it does not exist.
///
/// The common path is one `select`. The insert runs only when there is no row, and it is
/// `on conflict do nothing` rather than `do update`: a read that wrote an `upsert` row would
/// put a `change_log` entry in every device's pull for a screen the student merely opened,
/// and the `user_settings_log_change` trigger cannot tell that apart from a real edit.
pub async fn load_or_create(pool: &PgPool, user_id: Uuid) -> AppResult<SettingsRow> {
    if let Some(row) = select(pool, user_id).await? {
        return Ok(row);
    }

    let sql = format!(
        "insert into user_settings (user_id) values ($1)
         on conflict (user_id) do nothing
         returning {SETTINGS_COLUMNS}"
    );
    let created: Option<SettingsRow> = sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("creating a settings row: {error}")))?;

    match created {
        Some(row) => Ok(row),
        // Another request created it between the read and the insert, which is the race
        // `do nothing` exists for: read the row it wrote rather than reporting a conflict
        // the caller cannot act on.
        None => select(pool, user_id)
            .await?
            .ok_or_else(|| AppError::Internal(anyhow::anyhow!("a settings row vanished"))),
    }
}

/// The stored row, or `None` when the account has never had one.
async fn select(pool: &PgPool, user_id: Uuid) -> AppResult<Option<SettingsRow>> {
    let sql = format!("select {SETTINGS_COLUMNS} from user_settings where user_id = $1");
    sqlx::query_as(sqlx::AssertSqlSafe(sql))
        .bind(user_id)
        .fetch_optional(pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the settings row: {error}")))
}

/// The named settings fields, after the field rules have run.
///
/// `None` means the payload did not name the field; `Some(None)` means it named it as null,
/// which is how a pair like the quiet hours is cleared.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SettingsPatch {
    pub study_start_hour: Option<i32>,
    pub study_end_hour: Option<i32>,
    pub max_study_min_per_day: Option<i32>,
    pub block_len_min: Option<i32>,
    pub break_min: Option<i32>,
    pub reminder_channels: Option<Vec<ReminderChannel>>,
    pub quiet_start_hour: Option<Option<i32>>,
    pub quiet_end_hour: Option<Option<i32>>,
    pub language: Option<String>,
    pub theme: Option<String>,
    pub timezone: Option<String>,
}

/// The settings fields a client may write, which is the protocol's own allowlist plus
/// `timezone`.
///
/// `timezone` is the addition because it lives on `users` (A203) and this route is its only
/// writer; every other name is the same list the pull ships and the push writes, so the two
/// directions cannot disagree about what a settings payload is.
fn client_columns() -> Vec<&'static str> {
    let mut columns = Entity::UserSettings.payload_columns().to_vec();
    columns.push("timezone");
    columns
}

/// The field rules every writer of a settings row applies.
///
/// Validation runs **before** the write, so the migration's `check` constraints are a
/// backstop rather than the error path: a constraint violation would be a 500 from a
/// statement that had already run, while a rule here is the 400 the client can act on.
///
/// `is_create` is not a parameter, unlike the task and event validators: there is no create
/// for this entity — the row is keyed by the user and [`load_or_create`] writes it with the
/// column defaults — so every write is a partial one.
pub fn validate_settings(payload: &Value) -> Result<SettingsPatch, Reject> {
    let object = payload
        .as_object()
        .ok_or_else(|| Reject::validation("the payload must be a JSON object"))?;

    // A200: the field is not the client's to state. Refused rather than ignored, because a
    // forged payload that looks accepted is how a write lands on the wrong row.
    if object.contains_key("user_id") {
        return Err(Reject::validation(
            "user_id is not a field a client may write; the row is the caller's own",
        ));
    }
    let known = client_columns();
    for key in object.keys() {
        if !known.contains(&key.as_str()) {
            return Err(Reject::validation(format!(
                "{key} is not a field a client may write"
            )));
        }
    }

    let study_start_hour: Option<i32> = int_field(object, "study_start_hour")?;
    let study_end_hour: Option<i32> = int_field(object, "study_end_hour")?;
    let max_study_min_per_day: Option<i32> = int_field(object, "max_study_min_per_day")?;
    let block_len_min: Option<i32> = int_field(object, "block_len_min")?;
    let break_min: Option<i32> = int_field(object, "break_min")?;
    let quiet_start_hour: Option<Option<i32>> = nullable_int_field(object, "quiet_start_hour")?;
    let quiet_end_hour: Option<Option<i32>> = nullable_int_field(object, "quiet_end_hour")?;

    // The study-hours pair is only checked when the payload names both: a partial write that
    // changes one hour is a legitimate request, and `start < end` is a statement about the
    // pair the row will hold. The migration's constraint still holds the stored row to it.
    if let (Some(start), Some(end)) = (study_start_hour, study_end_hour) {
        if !(0..=23).contains(&start) {
            return Err(Reject::validation("study_start_hour must be 0-23"));
        }
        if !(1..=24).contains(&end) {
            return Err(Reject::validation("study_end_hour must be 1-24"));
        }
        if start >= end {
            return Err(Reject::validation(
                "study_start_hour must be before study_end_hour",
            ));
        }
    }
    if let Some(minutes) = max_study_min_per_day
        && !(15..=1440).contains(&minutes)
    {
        return Err(Reject::validation("max_study_min_per_day must be 15-1440"));
    }
    if let Some(minutes) = block_len_min
        && !(5..=180).contains(&minutes)
    {
        return Err(Reject::validation("block_len_min must be 5-180"));
    }
    if let Some(minutes) = break_min
        && !(0..=60).contains(&minutes)
    {
        return Err(Reject::validation("break_min must be 0-60"));
    }

    // Quiet hours are a pair or neither, and the pair is the **payload's**: a write that
    // names one hour has stated half a window, and the other half is a stored value the
    // client did not send. The window may wrap midnight, so there is no `start < end` rule —
    // `22 -> 7` is the normal case (A202).
    match (quiet_start_hour, quiet_end_hour) {
        (None, None) => {}
        (Some(_), Some(_)) => {}
        _ => {
            return Err(Reject::validation(
                "quiet_start_hour and quiet_end_hour are a pair: set both or neither",
            ));
        }
    }
    for (name, hour) in [
        ("quiet_start_hour", quiet_start_hour),
        ("quiet_end_hour", quiet_end_hour),
    ] {
        if let Some(Some(hour)) = hour
            && !(0..=23).contains(&hour)
        {
            return Err(Reject::validation(format!("{name} must be 0-23")));
        }
    }

    let language: Option<String> = text_field(object, "language")?;
    if let Some(language) = &language
        && !LANGUAGES.contains(&language.as_str())
    {
        return Err(Reject::validation(format!(
            "{language} is not a language; use one of {}",
            LANGUAGES.join(", ")
        )));
    }
    let theme: Option<String> = text_field(object, "theme")?;
    if let Some(theme) = &theme
        && !THEMES.contains(&theme.as_str())
    {
        return Err(Reject::validation(format!(
            "{theme} is not a theme; use one of {}",
            THEMES.join(", ")
        )));
    }

    // An unknown zone must be refused rather than stored: `chrono-tz` reads it as UTC, so
    // the student's day boundary would silently become Greenwich's and every date on the
    // Today view would be wrong for them (A6).
    let timezone: Option<String> = text_field(object, "timezone")?;
    if let Some(timezone) = &timezone
        && timezone.parse::<chrono_tz::Tz>().is_err()
    {
        return Err(Reject::validation(format!(
            "{timezone} is not an IANA timezone"
        )));
    }

    let reminder_channels: Option<Vec<ReminderChannel>> = match object.get("reminder_channels") {
        None | Some(Value::Null) => None,
        Some(value) => Some(
            serde_json::from_value(value.clone())
                .map_err(|error| Reject::validation(format!("reminder_channels: {error}")))?,
        ),
    };

    Ok(SettingsPatch {
        study_start_hour,
        study_end_hour,
        max_study_min_per_day,
        block_len_min,
        break_min,
        reminder_channels,
        quiet_start_hour,
        quiet_end_hour,
        language,
        theme,
        timezone,
    })
}

/// The settings rules a **push** additionally has to satisfy (A200).
///
/// Two things only the push path can get wrong, and both are silent:
///
///  * `user_settings`' primary key *is* `user_id` and the table has no `id` column, so the
///    contract's `Mutation.id` has nothing else to be. A mutation whose `id` is another
///    account's is refused rather than applied to the caller: the apply keys the row by the
///    session's user and ignores the mutation's id entirely, so without this check a forged
///    id would be accepted and silently do nothing, which looks exactly like success.
///  * A payload naming `user_id` is refused, not ignored, for the same reason — a field that
///    looks accepted while being dropped is how a forged value reaches a client's cache.
pub fn validate_settings_push(
    user_id: Uuid,
    mutation_id: Uuid,
    payload: &Value,
) -> Result<SettingsPatch, Reject> {
    if mutation_id != user_id {
        return Err(Reject::validation(
            "a settings mutation's id must be the caller's own user id",
        ));
    }
    validate_settings(payload)
}

fn int_field(object: &Map<String, Value>, key: &str) -> Result<Option<i32>, Reject> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Null) => Err(Reject::validation(format!("{key} must not be null"))),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| Reject::validation(format!("{key}: {error}"))),
    }
}

fn nullable_int_field(
    object: &Map<String, Value>,
    key: &str,
) -> Result<Option<Option<i32>>, Reject> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Null) => Ok(Some(None)),
        Some(value) => serde_json::from_value(value.clone())
            .map(|value| Some(Some(value)))
            .map_err(|error| Reject::validation(format!("{key}: {error}"))),
    }
}

fn text_field(object: &Map<String, Value>, key: &str) -> Result<Option<String>, Reject> {
    match object.get(key) {
        None => Ok(None),
        Some(Value::Null) => Err(Reject::validation(format!("{key} must not be null"))),
        Some(value) => serde_json::from_value(value.clone())
            .map(Some)
            .map_err(|error| Reject::validation(format!("{key}: {error}"))),
    }
}

/// `GET /settings`: the row and the account's timezone beside it (A203).
pub async fn read(State(state): State<AppState>, user: AuthUser) -> AppResult<AxumJson<Value>> {
    let settings = load_or_create(&state.pool, user.user_id).await?;
    Ok(AxumJson(json!({
        "settings": settings,
        "timezone": user.timezone,
        "channels_available": channels_available(&state),
    })))
}

/// The reminder channels this deployment can actually deliver on (A288).
///
/// **Derived from the transports that exist, not from the plan matrix**, and that distinction
/// is the whole point of the field: `config/plans.toml` grants `telegram` and `discord` to
/// Power, `PUT /settings` accepts them, and nothing sends them. A screen that offered a
/// channel which is granted, selectable and silently never fires is worse than one that shows
/// it as unavailable, because the student's only evidence is a reminder that never arrives.
///
/// The order is the matrix's own (`web, push, email`), so the screen's order is a contract
/// rather than whatever `HashMap` a future edit reaches for. `web` is unconditional: it is the
/// in-app feed and the desktop shell's local notification, and it needs no transport. `email`
/// is unconditional too — the mailer is always present, because a deployment with no SMTP
/// credentials gets the dev sink, which does deliver (A43's rule that a developer must be able
/// to exercise the product). `push` is the one that depends on configuration.
pub fn channels_available(state: &AppState) -> Vec<&'static str> {
    let mut channels = vec![crate::reminders::channel_str(ReminderChannel::Web)];
    if state.push.is_configured() {
        channels.push(crate::reminders::channel_str(ReminderChannel::Push));
    }
    channels.push(crate::reminders::channel_str(ReminderChannel::Email));
    channels
}

/// `PUT /settings`: a partial write, whole-row stored, channels checked before it happens.
///
/// The channel check goes through `entitlements::require(Feature::ReminderChannel(c))` and
/// never a `plans.get(..).reminder_channels` read: the matrix is the authority, a second
/// read of it here would be a second source of truth for what a plan grants, and the route
/// sweep forbids it (A201).
///
/// The refusal comes **before** the write, so a rejected channel leaves the row exactly as
/// it was: a write that saved `["web", "telegram"]` and then refused would leave the account
/// with a channel it cannot receive on.
pub async fn write(
    State(state): State<AppState>,
    user: AuthUser,
    AxumJson(body): AxumJson<Value>,
) -> AppResult<AxumJson<Value>> {
    let patch = validate_settings(&body).map_err(Reject::into_app_error)?;

    if let Some(channels) = &patch.reminder_channels {
        for channel in channels {
            crate::entitlements::require(&state, &user, Feature::ReminderChannel(*channel)).await?;
        }
    }

    // The row is created first, so the update below always has one to write: a `PUT` on an
    // account that has never opened the screen is a normal first save, not a 404.
    let mut tx = state.pool.begin().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("beginning a settings write: {error}"))
    })?;
    load_or_create_in(&mut tx, user.user_id).await?;

    // The whole row is written from the merged values, which is what keeps `user_settings`
    // record-level LWW (A201): a per-field merge would need a per-field timestamp the table
    // does not have.
    let stored: SettingsRow = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "update user_settings set
            study_start_hour      = coalesce($2, study_start_hour),
            study_end_hour        = coalesce($3, study_end_hour),
            max_study_min_per_day = coalesce($4, max_study_min_per_day),
            block_len_min         = coalesce($5, block_len_min),
            break_min             = coalesce($6, break_min),
            reminder_channels     = coalesce($7, reminder_channels),
            quiet_start_hour      = case when $8::bool then $9::int else quiet_start_hour end,
            quiet_end_hour        = case when $10::bool then $11::int else quiet_end_hour end,
            language              = coalesce($12, language),
            theme                 = coalesce($13, theme)
          where user_id = $1
         returning {SETTINGS_COLUMNS}"
    )))
    .bind(user.user_id)
    .bind(patch.study_start_hour)
    .bind(patch.study_end_hour)
    .bind(patch.max_study_min_per_day)
    .bind(patch.block_len_min)
    .bind(patch.break_min)
    .bind(
        patch
            .reminder_channels
            .as_ref()
            .map(|channels| Json(channels.clone())),
    )
    // The quiet hours need the two-argument form: `coalesce` cannot express "set it to null"
    // because null is exactly what it skips, so the pair travels as a flag plus a value.
    .bind(patch.quiet_start_hour.is_some())
    .bind(patch.quiet_start_hour.flatten())
    .bind(patch.quiet_end_hour.is_some())
    .bind(patch.quiet_end_hour.flatten())
    .bind(patch.language.as_deref())
    .bind(patch.theme.as_deref())
    .fetch_one(&mut *tx)
    .await
    .map_err(|error| {
        // A constraint the rules above missed is still the body's fault, not a server
        // fault: 400 with the database's own message rather than a 500.
        if crate::sync::apply::client_error(&error).is_some() {
            AppError::Validation("a settings value is out of range".to_string())
        } else {
            AppError::Internal(anyhow::anyhow!("writing the settings row: {error}"))
        }
    })?;

    // The timezone is on `users` (A203), so this is the only writer of it outside signup.
    let timezone = match patch.timezone {
        Some(timezone) => {
            sqlx::query("update users set timezone = $2 where id = $1")
                .bind(user.user_id)
                .bind(&timezone)
                .execute(&mut *tx)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("writing the timezone: {error}"))
                })?;
            timezone
        }
        None => user.timezone.clone(),
    };

    tx.commit().await.map_err(|error| {
        AppError::Internal(anyhow::anyhow!("committing a settings write: {error}"))
    })?;

    Ok(AxumJson(
        json!({ "settings": stored, "timezone": timezone }),
    ))
}

/// [`load_or_create`] on a connection the caller owns, so a write can create the row in the
/// same transaction that updates it.
async fn load_or_create_in(conn: &mut PgConnection, user_id: Uuid) -> AppResult<()> {
    sqlx::query("insert into user_settings (user_id) values ($1) on conflict (user_id) do nothing")
        .bind(user_id)
        .execute(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("creating a settings row: {error}")))?;
    Ok(())
}
