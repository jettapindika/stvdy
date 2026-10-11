//! The background job queue: a table, a `Notify`, and one polling worker.
//!
//! A10 fixes the design: no Redis, `FOR UPDATE SKIP LOCKED`, `run_at` for
//! scheduling, `attempts` with exponential backoff, and `dedupe_key` for
//! singleton recurring jobs so a restart cannot double-run a credit grant.
//!
//! Two failure modes get explicit handling, because both silently park work:
//! a job whose handler panics (the row must not stay `running` forever), and a
//! process that dies mid-job (the boot sweep requeues anything left `running`).

pub mod handlers;
pub mod worker;

use std::sync::Arc;
use std::time::Duration;

use serde_json::Value;
use sqlx::{PgPool, Row};
use tokio::sync::Notify;

/// How many times a job is retried before it is marked `failed`. The backoff is
/// `2^attempts` seconds, so five attempts span roughly half a minute.
pub const MAX_ATTEMPTS: i32 = 5;

/// How often the worker looks for due jobs. The `Notify` makes an enqueue
/// immediate; this is the floor for `run_at` scheduling and for a job that was
/// retried with a delay.
pub const POLL_INTERVAL: Duration = Duration::from_secs(5);

#[derive(Clone)]
pub struct Queue {
    notify: Arc<Notify>,
}

impl Queue {
    pub fn new() -> Self {
        Self {
            notify: Arc::new(Notify::new()),
        }
    }

    /// Wakes the worker. Called after an enqueue and after a retry is scheduled.
    pub fn wake(&self) {
        self.notify.notify_one();
    }

    pub fn notifier(&self) -> Arc<Notify> {
        self.notify.clone()
    }
}

impl Default for Queue {
    fn default() -> Self {
        Self::new()
    }
}

/// Inserts a job and wakes the worker.
///
/// `dedupe_key` is set only for singleton recurring jobs. The unique index on
/// `(dedupe_key) where dedupe_key is not null` means a second enqueue with the
/// same key is a no-op rather than an error, which is what makes it safe to
/// enqueue from a boot path.
pub async fn enqueue(
    pool: &PgPool,
    queue: &Queue,
    kind: &str,
    payload: Value,
    dedupe_key: Option<&str>,
) -> sqlx::Result<uuid::Uuid> {
    let id: uuid::Uuid = sqlx::query_scalar(
        // The `where` clause is required: the unique index on `dedupe_key` is
        // partial (`where dedupe_key is not null`), and Postgres will not infer
        // a partial index as a conflict target without the same predicate.
        "insert into jobs (kind, payload, dedupe_key)
         values ($1, $2, $3)
         on conflict (dedupe_key) where dedupe_key is not null
           do update set payload = excluded.payload
         returning id",
    )
    .bind(kind)
    .bind(payload)
    .bind(dedupe_key)
    .fetch_one(pool)
    .await?;

    queue.wake();
    Ok(id)
}

/// Why a job failed, and whether retrying it could ever help.
///
/// The distinction is not cosmetic: a missing pdfium library or an unsupported
/// source kind will fail identically on every attempt, and retrying it five times
/// with exponential backoff leaves the source `processing` for half a minute and
/// burns the queue behind it. Only a transient failure is worth a retry.
#[derive(Debug, Clone)]
pub struct Failure {
    pub message: String,
    pub retryable: bool,
}

impl Failure {
    /// A failure that a retry might fix: a provider hiccup, a timeout, a lock.
    pub fn transient(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: true,
        }
    }

    /// A failure that will repeat: a missing binary, an unsupported format, a
    /// document that cannot be parsed. Retrying it only delays the answer.
    pub fn permanent(message: impl Into<String>) -> Self {
        Self {
            message: message.into(),
            retryable: false,
        }
    }
}

impl std::fmt::Display for Failure {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

/// The job kinds this phase dispatches. Kept as a match on the `kind` column
/// rather than an enum in the payload so a new kind is a handler plus a row,
/// and an unknown kind is a `failed` job with a readable error instead of a
/// panic in the worker.
pub struct ClaimedJob {
    pub id: uuid::Uuid,
    pub kind: String,
    pub payload: Value,
    pub attempts: i32,
}

/// Claims one due job, if any. `SKIP LOCKED` is what lets several workers (or
/// several instances) run without contending on the same row.
pub async fn claim(pool: &PgPool) -> sqlx::Result<Option<ClaimedJob>> {
    let row = sqlx::query(
        "update jobs
            set status = 'running', attempts = attempts + 1
          where id = (
                select id from jobs
                 where status = 'queued' and run_at <= now()
                 order by run_at
                 for update skip locked
                 limit 1)
      returning id, kind, payload, attempts",
    )
    .fetch_optional(pool)
    .await?;

    Ok(row.map(|row| ClaimedJob {
        id: row.get("id"),
        kind: row.get("kind"),
        payload: row.get("payload"),
        attempts: row.get("attempts"),
    }))
}

pub async fn mark_done(pool: &PgPool, id: uuid::Uuid) -> sqlx::Result<()> {
    sqlx::query("update jobs set status = 'done', error = null where id = $1")
        .bind(id)
        .execute(pool)
        .await?;
    Ok(())
}

/// Records a failure and either schedules a retry or gives up.
///
/// The error text is stored on the row because it is what the user sees in
/// `GET /sources/:id/status`: "pdfium is not available" is actionable, "internal
/// error" is not.
pub async fn mark_failed(
    pool: &PgPool,
    id: uuid::Uuid,
    attempts: i32,
    failure: &Failure,
) -> sqlx::Result<bool> {
    if !failure.retryable || attempts >= MAX_ATTEMPTS {
        sqlx::query("update jobs set status = 'failed', error = $2 where id = $1")
            .bind(id)
            .bind(&failure.message)
            .execute(pool)
            .await?;
        return Ok(false);
    }

    let backoff_secs = 1_i64 << attempts.min(10);
    sqlx::query(
        "update jobs
            set status = 'queued',
                error = $2,
                run_at = now() + make_interval(secs => $3)
          where id = $1",
    )
    .bind(id)
    .bind(&failure.message)
    .bind(backoff_secs as f64)
    .execute(pool)
    .await?;
    Ok(true)
}

/// Requeues jobs left `running` by a process that died. Called once at boot: a
/// crashed job would otherwise sit `running` forever, and the source it belongs
/// to would stay `processing` with no way out.
pub async fn requeue_orphans(pool: &PgPool) -> sqlx::Result<u64> {
    let result = sqlx::query(
        "update jobs
            set status = 'queued',
                run_at = now(),
                error = 'requeued after a restart'
          where status = 'running'",
    )
    .execute(pool)
    .await?;
    Ok(result.rows_affected())
}
