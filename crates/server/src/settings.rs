//! `GET /settings` and `PUT /settings`.
//!
//! The row is **created on first read** ([`load_or_create`]), so a read never has to answer
//! "no settings yet" and a write never has to decide whether to insert. A screen that has
//! one shape is a screen whose loading and empty states cannot disagree.
//!
//! The timezone is deliberately **not** on this row (A203): `users.timezone` predates any
//! settings row, the reminder and planner maths read it for an account that has never
//! opened this screen, and a second copy here is exactly the drift the "local-date maths
//! only through `chrono-tz`" rule would not catch. `GET /settings` therefore returns it
//! beside the row rather than inside it, and `PUT /settings` writes it to `users`.

use chrono::{DateTime, Utc};
use sqlx::PgPool;
use sqlx::types::Json;
use uuid::Uuid;

use stvdy_core::config::ReminderChannel;

use crate::error::{AppError, AppResult};

/// The columns every read and every returning write selects, in one place so a column added
/// to the table and forgotten here is a compile error rather than a silently missing field.
const SETTINGS_COLUMNS: &str = "study_start_hour, study_end_hour, max_study_min_per_day, \
                                block_len_min, break_min, reminder_channels, \
                                quiet_start_hour, quiet_end_hour, language, theme, updated_at";

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
/// `on conflict do nothing` rather than `do update`: a read that wrote an `upsert` row
/// would put a `change_log` entry in every device's pull for a screen the student merely
/// opened, and the `user_settings_log_change` trigger cannot tell that apart from a real
/// edit.
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
