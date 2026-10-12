//! `compact_change_log`: the two retention windows, and the watermark they create.
//!
//! The change log grows without bound otherwise: every write on every device appends a
//! row. Compaction deletes by age, and the *reason* there are two windows is that a
//! tombstone and an upsert are not the same kind of fact (A184). An upsert older than the
//! retention window is redundant — the row it describes is already on every device that
//! cares. A `delete` is not: a device that has been offline for three months must still be
//! told that a row was removed rather than silently keep its stale copy, so tombstones are
//! kept for at least as long as the window.
//!
//! The job also writes the per-user watermark, which is what makes a stale cursor
//! answerable: the seq below which this account's history is gone (A165).

use sqlx::PgConnection;
use uuid::Uuid;

use crate::jobs::{ClaimedJob, Failure};
use crate::state::AppState;

/// The two retention windows, in days.
#[derive(Debug, Clone, Copy)]
pub struct Retention {
    /// How long an `upsert` change is kept.
    pub upsert_days: i32,
    /// How long a `delete` change is kept. At least `upsert_days`.
    pub tombstone_days: i32,
}

/// Runs one compaction pass over every live account.
pub async fn run(state: &AppState, _job: &ClaimedJob) -> Result<(), Failure> {
    let retention = Retention {
        upsert_days: state.config.change_log_retention_days as i32,
        tombstone_days: state.config.change_log_tombstone_retention_days as i32,
    };

    let accounts: Vec<Uuid> = sqlx::query_scalar("select id from users where deleted_at is null")
        .fetch_all(&state.pool)
        .await
        .map_err(|error| Failure::transient(format!("listing accounts: {error}")))?;

    let mut compacted = 0u64;
    for user_id in accounts.iter().copied() {
        // One transaction per account: the delete and the watermark are one fact, so a
        // crash between them cannot leave a cursor told it is stale while the rows it
        // needs are still there, or vice versa.
        let mut tx = state
            .pool
            .begin()
            .await
            .map_err(|error| Failure::transient(format!("beginning a compaction: {error}")))?;
        let outcome = compact_user(&mut tx, user_id, retention).await;
        let outcome = outcome
            .map_err(|error| Failure::transient(format!("compacting {user_id}: {error}")))?;
        tx.commit()
            .await
            .map_err(|error| Failure::transient(format!("committing a compaction: {error}")))?;
        if outcome.is_some() {
            compacted += 1;
        }
    }

    tracing::info!(
        accounts = accounts.len(),
        compacted,
        upsert_days = retention.upsert_days,
        tombstone_days = retention.tombstone_days,
        "change log compaction"
    );
    Ok(())
}

/// Compacts one account's change log, in the caller's transaction.
///
/// Returns the watermark it wrote, or `None` when nothing was old enough to remove — in
/// which case the watermark is left alone. That matters: an account whose whole history is
/// inside the window has no gap, so writing `min(seq)` as a watermark would tell a device
/// holding an older cursor to resync for no reason.
pub async fn compact_user(
    conn: &mut PgConnection,
    user_id: Uuid,
    retention: Retention,
) -> sqlx::Result<Option<i64>> {
    let upserts = sqlx::query(
        "delete from change_log
          where user_id = $1 and op = 'upsert'
            and changed_at < now() - make_interval(days => $2)",
    )
    .bind(user_id)
    .bind(retention.upsert_days)
    .execute(&mut *conn)
    .await?
    .rows_affected();

    let tombstones = sqlx::query(
        "delete from change_log
          where user_id = $1 and op = 'delete'
            and changed_at < now() - make_interval(days => $2)",
    )
    .bind(user_id)
    .bind(retention.tombstone_days)
    .execute(&mut *conn)
    .await?
    .rows_affected();

    if upserts + tombstones == 0 {
        return Ok(None);
    }

    // The first seq that survived. A cursor below it points at history that is gone, so
    // `is_stale` tells that device to resync from zero; a cursor at or above it is still
    // servable. `greatest` keeps a watermark from moving backwards if a later pass finds
    // fewer rows (it cannot today, but the invariant is cheap to hold).
    let min_retained_seq: i64 =
        sqlx::query_scalar("select coalesce(min(seq), 0) from change_log where user_id = $1")
            .bind(user_id)
            .fetch_one(&mut *conn)
            .await?;

    sqlx::query(
        "insert into change_log_watermark (user_id, min_retained_seq, compacted_at)
         values ($1, $2, now())
         on conflict (user_id) do update
            set min_retained_seq =
                    greatest(change_log_watermark.min_retained_seq, excluded.min_retained_seq),
                compacted_at = excluded.compacted_at",
    )
    .bind(user_id)
    .bind(min_retained_seq)
    .execute(&mut *conn)
    .await?;

    Ok(Some(min_retained_seq))
}
