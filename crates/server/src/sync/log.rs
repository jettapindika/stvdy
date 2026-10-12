//! The idempotency log and the seq of a change.
//!
//! `mutation_log` is what makes a replayed mutation apply nothing (the exit gate's
//! second clause): a `mutation_id` already recorded returns the stored result and the
//! batch moves on. `mutation_id` is a **global** primary key, so a collision with a
//! different account is representable — and returning that account's stored result
//! would be a cross-tenant read on the one route this phase is most worried about
//! (A172).

use sqlx::PgConnection;
use stvdy_sync::proto::{Entity, MutationResult};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

/// What the idempotency log knows about a `mutation_id`.
pub enum Replay {
    /// This account already applied it: return the stored result and apply nothing.
    Stored(MutationResult),
    /// The id is recorded for a **different** account (A172). A conflict, never that
    /// account's result.
    ForeignUser,
    /// Not seen before.
    Fresh,
}

/// Looks up a `mutation_id` in the caller's own idempotency log.
pub async fn replay(
    conn: &mut PgConnection,
    user_id: Uuid,
    mutation_id: Uuid,
) -> AppResult<Replay> {
    let row: Option<(Uuid, serde_json::Value)> =
        sqlx::query_as("select user_id, result from mutation_log where mutation_id = $1")
            .bind(mutation_id)
            .fetch_optional(conn)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("reading the idempotency log: {error}"))
            })?;

    let Some((stored_user, result)) = row else {
        return Ok(Replay::Fresh);
    };
    if stored_user != user_id {
        return Ok(Replay::ForeignUser);
    }
    let result: MutationResult = serde_json::from_value(result).map_err(|error| {
        AppError::Internal(anyhow::anyhow!(
            "decoding a stored mutation result: {error}"
        ))
    })?;
    Ok(Replay::Stored(result))
}

/// Records a mutation's result.
///
/// `on conflict do nothing` because a concurrent identical replay can reach here with
/// the row already written; the stored value is the same one this call would write, so
/// the loser of the race is a no-op rather than an error.
pub async fn record(
    conn: &mut PgConnection,
    user_id: Uuid,
    device_id: Uuid,
    mutation_id: Uuid,
    result: &MutationResult,
) -> AppResult<()> {
    let result = serde_json::to_value(result).map_err(|error| {
        AppError::Internal(anyhow::anyhow!("encoding a mutation result: {error}"))
    })?;
    sqlx::query(
        "insert into mutation_log (mutation_id, user_id, device_id, result)
         values ($1, $2, $3, $4)
         on conflict (mutation_id) do nothing",
    )
    .bind(mutation_id)
    .bind(user_id)
    .bind(device_id)
    .bind(result)
    .execute(conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing the idempotency log: {error}")))?;
    Ok(())
}

/// The seq of the row's latest `change_log` entry, or 0 when it has none.
///
/// Used for an append-only duplicate: the insert is ignored, so the response reports
/// the existing change rather than a new one, and the client's cursor does not move for
/// a write that changed nothing (A174).
pub async fn seq_for(
    conn: &mut PgConnection,
    user_id: Uuid,
    entity: Entity,
    entity_id: Uuid,
) -> AppResult<i64> {
    sqlx::query_scalar(
        "select coalesce(max(seq), 0) from change_log
          where user_id = $1 and entity = $2 and entity_id = $3",
    )
    .bind(user_id)
    .bind(entity.table_name())
    .bind(entity_id)
    .fetch_one(conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a row's seq: {error}")))
}
