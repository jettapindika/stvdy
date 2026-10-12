//! Applying one mutation, and the one decision that is not Phase 7's.
//!
//! [`crate::sync::push::decide`] answers *whether* a mutation may be applied; this module
//! applies it. Every write is scoped by `user_id` **in the statement that writes**, never
//! by a `select` followed by a check in Rust: a check-then-act is interleavable by a
//! second device, and the ownership rule has to hold under exactly that (A173).
//!
//! A row travels as `jsonb` and is expanded by `jsonb_populate_record`, so the columns a
//! mutation writes come from [`Entity::payload_columns`] — the same allowlist a pull
//! ships — and a column added to a table needs no new Rust binding. Only the columns the
//! payload actually carries are written, so a column the client did not send keeps its
//! stored value (or its table default on an insert).

use std::collections::BTreeSet;

use serde_json::{Map, Value, json};
use sqlx::PgConnection;
use stvdy_sync::client::{Rule, rule_for};
use stvdy_sync::proto::{Entity, ErrorCode, Mutation, MutationResult, Op, PayloadShape};
use uuid::Uuid;

use crate::error::{AppError, AppResult};

use super::log;

/// The result of applying one mutation, before it becomes the wire value.
pub enum Outcome {
    Applied { seq: i64 },
    Rejected { code: ErrorCode, message: String },
    Conflict { winner: Value },
}

impl Outcome {
    /// The exact wire value the batch returns.
    pub fn into_result(self) -> MutationResult {
        match self {
            Outcome::Applied { seq } => MutationResult::Applied { seq },
            Outcome::Rejected { code, message } => MutationResult::Rejected {
                code: code.as_str().to_string(),
                message,
            },
            Outcome::Conflict { winner } => MutationResult::Conflict { winner },
        }
    }
}

/// Applies one mutation that `decide` has already allowed.
///
/// Dispatch is by the conflict table (A175), so the server and the client cannot disagree
/// about which rule an entity follows: an append-only row is inserted and never updated; a
/// `Task`/`Event` gets the field-level merge; a `user_settings` row is keyed by the caller's
/// user id.
pub async fn apply_one(
    conn: &mut PgConnection,
    user_id: Uuid,
    mutation: &Mutation,
) -> AppResult<Outcome> {
    match rule_for(mutation.entity) {
        Rule::AppendOnly => apply_append_only(conn, user_id, mutation).await,
        Rule::LwwWithFieldMerge => apply_patch(conn, user_id, mutation).await,
        Rule::Lww if mutation.entity == Entity::UserSettings => {
            apply_settings(conn, user_id, mutation).await
        }
        // A server-owned entity never reaches the apply: `decide` refuses it first. The arm
        // exists so the match is exhaustive, and a plain record-level write is the answer
        // that changes nothing if one ever did.
        Rule::Lww => apply_upsert(conn, user_id, mutation).await,
    }
}

/// The values a mutation carries, as a JSON object.
///
/// A `Task`/`Event` mutation carries `{"patch": …, "base": …}` (A176); every other entity
/// carries the row. A payload that is not an object is not a row this protocol
/// understands, and the caller refuses it rather than writing nulls.
fn row_of(mutation: &Mutation) -> Value {
    let payload = mutation.payload.clone().unwrap_or(Value::Null);
    match mutation.entity.payload_shape() {
        PayloadShape::PatchWithBase => payload.get("patch").cloned().unwrap_or(payload),
        PayloadShape::Row => payload,
    }
}

/// The stored row as JSON, or `None` when this account has no such row.
///
/// Scoped by `user_id` in the statement, which is what makes an id belonging to another
/// account indistinguishable from a missing one (A173).
async fn stored_row(
    conn: &mut PgConnection,
    user_id: Uuid,
    entity: Entity,
    id: Uuid,
) -> AppResult<Option<Value>> {
    let table = entity.table_name();
    let sql = if entity == Entity::UserSettings {
        format!("select to_jsonb(t) from {table} t where t.user_id = $1")
    } else {
        format!("select to_jsonb(t) from {table} t where t.id = $1 and t.user_id = $2")
    };
    let row: Option<Value> = if entity == Entity::UserSettings {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(user_id)
            .fetch_optional(&mut *conn)
            .await
    } else {
        sqlx::query_scalar(sqlx::AssertSqlSafe(sql))
            .bind(id)
            .bind(user_id)
            .fetch_optional(&mut *conn)
            .await
    }
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading a stored row: {error}")))?;
    Ok(row)
}

/// The columns a statement will write: the identity, then the allowlisted columns the
/// payload actually carries.
///
/// The intersection with [`Entity::payload_columns`] is what keeps a payload from naming
/// a column outside the allowlist (and so outside the protocol's contract). `id` and
/// `user_id` are always written, from the mutation and the session rather than from the
/// payload: a body free to rewrite an id could point a write at another account's row.
fn write_columns(entity: Entity, row: &Value) -> Vec<&'static str> {
    // `user_settings` is keyed by `user_id` and has no `id` column at all.
    let mut columns = if entity == Entity::UserSettings {
        vec!["user_id"]
    } else {
        vec!["id", "user_id"]
    };
    if let Some(object) = row.as_object() {
        columns.extend(
            entity
                .payload_columns()
                .iter()
                .copied()
                .filter(|column| *column != "id" && object.contains_key(*column)),
        );
    }
    columns
}

/// `insert … on conflict (id) do update set … where <table>.user_id = excluded.user_id`.
///
/// The `where` on the conflict is the ownership check, in the statement: a row whose id
/// belongs to another account matches the conflict and then fails the guard, so the
/// statement affects nothing and the caller reports `not_found` (A173).
async fn write_row(
    conn: &mut PgConnection,
    entity: Entity,
    row: &Value,
) -> AppResult<sqlx::postgres::PgQueryResult> {
    let table = entity.table_name();
    let columns = write_columns(entity, row);
    let sets = columns
        .iter()
        .filter(|column| **column != "id" && **column != "user_id")
        .map(|column| format!("{column} = excluded.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    let sql = format!(
        "insert into {table} ({cols})
         select {cols} from jsonb_populate_record(null::{table}, $1::jsonb)
         on conflict (id) do update set {sets}
         where {table}.user_id = excluded.user_id",
        cols = columns.join(", "),
    );
    run_statement(sqlx::query(sqlx::AssertSqlSafe(sql)).bind(row), conn).await
}

/// An append-only insert: a duplicate id is ignored rather than overwriting (A174).
async fn insert_append_only(
    conn: &mut PgConnection,
    entity: Entity,
    row: &Value,
) -> AppResult<sqlx::postgres::PgQueryResult> {
    let table = entity.table_name();
    let columns = write_columns(entity, row);
    let sql = format!(
        "insert into {table} ({cols})
         select {cols} from jsonb_populate_record(null::{table}, $1::jsonb)
         on conflict (id) do nothing",
        cols = columns.join(", "),
    );
    run_statement(sqlx::query(sqlx::AssertSqlSafe(sql)).bind(row), conn).await
}

/// Runs a statement, turning a client-caused database error into `AppError::Validation`.
///
/// A constraint or data error (class 22/23) is the payload — a missing required column, a
/// bad uuid, a dangling reference — so it must become a per-mutation rejection rather than
/// a 500 that aborts the whole batch. Every other error is a server fault.
async fn run_statement<'q>(
    query: sqlx::query::Query<'q, sqlx::Postgres, sqlx::postgres::PgArguments>,
    conn: &mut PgConnection,
) -> AppResult<sqlx::postgres::PgQueryResult> {
    match query.execute(&mut *conn).await {
        Ok(result) => Ok(result),
        Err(error) => match client_error(&error) {
            Some(message) => Err(AppError::Validation(message)),
            None => Err(AppError::Internal(anyhow::anyhow!(
                "applying a mutation: {error}"
            ))),
        },
    }
}

/// The message for a database error the client caused, or `None` for a server fault.
fn client_error(error: &sqlx::Error) -> Option<String> {
    let database = error.as_database_error()?;
    let code = database.code()?;
    // Class 22 is a data exception, class 23 an integrity constraint: a missing required
    // column, a bad uuid, a dangling reference. All of them are the payload.
    if code.starts_with("22") || code.starts_with("23") {
        Some(database.message().to_string())
    } else {
        None
    }
}

/// A statement result, with a client-caused error turned into a rejection.
///
/// The `?` on a writer call is therefore not enough on its own: a `Validation` error from
/// the payload is a *result* for that one mutation, not a failure of the batch. Every
/// caller of a writer goes through this.
#[allow(clippy::type_complexity)]
fn as_outcome(
    result: AppResult<sqlx::postgres::PgQueryResult>,
) -> AppResult<Result<sqlx::postgres::PgQueryResult, Outcome>> {
    match result {
        Ok(result) => Ok(Ok(result)),
        Err(AppError::Validation(message)) => Ok(Err(Outcome::Rejected {
            code: ErrorCode::Validation,
            message,
        })),
        Err(error) => Err(error),
    }
}

/// The seq of the change the trigger just logged for this row.
async fn seq_after(
    conn: &mut PgConnection,
    user_id: Uuid,
    entity: Entity,
    id: Uuid,
) -> AppResult<i64> {
    log::seq_for(conn, user_id, entity, id).await
}

/// The refusal every writer shares when the payload carries no writable field.
fn empty_payload() -> Outcome {
    Outcome::Rejected {
        code: ErrorCode::Validation,
        message: "the payload carries no field for this entity".to_string(),
    }
}

/// How a patch and the row it was edited from combine with what the server now holds.
#[derive(Debug)]
pub enum Merge {
    /// The other writer's changed set and this patch's are disjoint: the merged row.
    Disjoint(Value),
    /// They overlap, or there is no base to compare against: the patch wins whole.
    Overlap,
    /// The stored row is tombstoned and this patch does not restore it.
    Tombstone,
}

/// The merge rule for `tasks` and `events` (A177).
///
/// The other writer's changed set is `{k : stored[k] != base[k]}` and the client's is
/// `keys(patch)`. Disjoint sets mean the two edits touched different fields, so both
/// survive; an overlap means they touched the same field, and the incoming patch wins whole
/// because a per-field tie-break would need a per-field clock the schema does not have.
///
/// A tombstoned row wins over an edit, unless the patch explicitly restores it with
/// `deleted_at: null` (A178) — the one exception the contract names.
pub fn merge_patch(stored: &Value, base: Option<&Value>, patch: &Value) -> Merge {
    let tombstoned = stored
        .get("deleted_at")
        .is_some_and(|value| !value.is_null());
    let restores = patch.get("deleted_at").is_some_and(Value::is_null);
    if tombstoned && !restores {
        return Merge::Tombstone;
    }

    // No base (a create, or a client that did not send one) or an explicit restore: there
    // is nothing to compare against, so the patch wins whole.
    let (Some(base), Some(stored), Some(patch)) = (
        base.and_then(Value::as_object),
        stored.as_object(),
        patch.as_object(),
    ) else {
        return Merge::Overlap;
    };
    if restores {
        return Merge::Overlap;
    }

    // `updated_at` is excluded: the trigger moves it on every write, so comparing it would
    // read as a change on every mutation and force every edit to look like an overlap.
    let other: BTreeSet<&str> = stored
        .iter()
        .filter(|(key, value)| {
            key.as_str() != "updated_at" && base.get(key.as_str()).is_none_or(|base| base != *value)
        })
        .map(|(key, _)| key.as_str())
        .collect();

    if patch.keys().all(|key| !other.contains(key.as_str())) {
        Merge::Disjoint(over(stored, patch))
    } else {
        Merge::Overlap
    }
}

/// The stored row with the patch's keys applied over it.
fn over(stored: &Map<String, Value>, patch: &Map<String, Value>) -> Value {
    let mut merged = stored.clone();
    for (key, value) in patch {
        merged.insert(key.clone(), value.clone());
    }
    Value::Object(merged)
}

/// A `Task`/`Event` mutation: a patch plus the base it was edited from (A176).
async fn apply_patch(
    conn: &mut PgConnection,
    user_id: Uuid,
    mutation: &Mutation,
) -> AppResult<Outcome> {
    if mutation.op == Op::Delete {
        return apply_delete(conn, user_id, mutation).await;
    }

    let entity = mutation.entity;
    let payload = mutation.payload.clone().unwrap_or(Value::Null);
    let patch = payload.get("patch").cloned().unwrap_or(payload.clone());
    let base = payload.get("base").filter(|value| !value.is_null());
    if !patch.is_object() {
        return Ok(empty_payload());
    }

    let stored = stored_row(conn, user_id, entity, mutation.id).await?;
    let mut row = match &stored {
        // A create: there is nothing to merge against, so the patch is the row.
        None => patch.clone(),
        Some(stored) => match merge_patch(stored, base, &patch) {
            Merge::Tombstone => {
                return Ok(Outcome::Conflict {
                    winner: stored.clone(),
                });
            }
            Merge::Disjoint(merged) => merged,
            // The patch wins whole: apply its keys over the stored row, so a field the
            // patch does not name keeps its stored value.
            Merge::Overlap => match (stored.as_object(), patch.as_object()) {
                (Some(stored), Some(patch)) => over(stored, patch),
                _ => patch.clone(),
            },
        },
    };
    if let Some(object) = row.as_object_mut() {
        object.insert("id".to_string(), json!(mutation.id));
        object.insert("user_id".to_string(), json!(user_id));
    }
    if write_columns(entity, &row).len() <= 2 {
        return Ok(empty_payload());
    }

    let result = match as_outcome(write_row(conn, entity, &row).await)? {
        Ok(result) => result,
        Err(outcome) => return Ok(outcome),
    };
    if result.rows_affected() == 0 {
        // The id exists and belongs to another account: the conflict guard refused it.
        return Ok(Outcome::Rejected {
            code: ErrorCode::NotFound,
            message: "no such row for this account".to_string(),
        });
    }
    Ok(Outcome::Applied {
        seq: seq_after(conn, user_id, entity, mutation.id).await?,
    })
}

/// Insert-or-update a mutable, id-keyed entity that carries its whole row.
///
/// The plain path: an entity whose `Rule` is `Lww` without a patch shape. Today that is
/// only a server-owned entity reaching the apply's fallback arm.
async fn apply_upsert(
    conn: &mut PgConnection,
    user_id: Uuid,
    mutation: &Mutation,
) -> AppResult<Outcome> {
    let entity = mutation.entity;
    let mut row = row_of(mutation);
    let Some(object) = row.as_object_mut() else {
        return Ok(empty_payload());
    };
    object.insert("id".to_string(), json!(mutation.id));
    object.insert("user_id".to_string(), json!(user_id));

    // Only `id`/`user_id` would mean an update that sets nothing: the statement would
    // have an empty `set`, so refuse it before it reaches Postgres.
    if write_columns(entity, &row).len() <= 2 {
        return Ok(empty_payload());
    }

    let result = match as_outcome(write_row(conn, entity, &row).await)? {
        Ok(result) => result,
        Err(outcome) => return Ok(outcome),
    };
    if result.rows_affected() == 0 {
        // The id exists and belongs to another account: the conflict guard refused it.
        return Ok(Outcome::Rejected {
            code: ErrorCode::NotFound,
            message: "no such row for this account".to_string(),
        });
    }
    Ok(Outcome::Applied {
        seq: seq_after(conn, user_id, entity, mutation.id).await?,
    })
}

/// A tombstone: set `deleted_at` and let the trigger log a `delete`.
async fn apply_delete(
    conn: &mut PgConnection,
    user_id: Uuid,
    mutation: &Mutation,
) -> AppResult<Outcome> {
    let table = mutation.entity.table_name();
    let sql = format!("update {table} set deleted_at = now() where id = $1 and user_id = $2");
    let result = sqlx::query(sqlx::AssertSqlSafe(sql))
        .bind(mutation.id)
        .bind(user_id)
        .execute(&mut *conn)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("applying a tombstone: {error}")))?;
    if result.rows_affected() == 0 {
        return Ok(Outcome::Rejected {
            code: ErrorCode::NotFound,
            message: "no such row for this account".to_string(),
        });
    }
    Ok(Outcome::Applied {
        seq: seq_after(conn, user_id, mutation.entity, mutation.id).await?,
    })
}

/// An append-only row: inserted once, and a duplicate reports the existing seq (A174).
async fn apply_append_only(
    conn: &mut PgConnection,
    user_id: Uuid,
    mutation: &Mutation,
) -> AppResult<Outcome> {
    let entity = mutation.entity;
    let mut row = row_of(mutation);
    let Some(object) = row.as_object_mut() else {
        return Ok(empty_payload());
    };
    object.insert("id".to_string(), json!(mutation.id));
    object.insert("user_id".to_string(), json!(user_id));

    let result = match as_outcome(insert_append_only(conn, entity, &row).await)? {
        Ok(result) => result,
        Err(outcome) => return Ok(outcome),
    };
    if result.rows_affected() == 1 {
        return Ok(Outcome::Applied {
            seq: seq_after(conn, user_id, entity, mutation.id).await?,
        });
    }

    // Nothing inserted: either the id is already this account's (report its existing seq,
    // so a retried review does not read as a new change), or it is another account's,
    // which is `not_found` and never a write.
    let exists: Option<i64> = sqlx::query_scalar(sqlx::AssertSqlSafe(format!(
        "select 1::bigint from {} where id = $1 and user_id = $2",
        entity.table_name()
    )))
    .bind(mutation.id)
    .bind(user_id)
    .fetch_optional(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("checking an append-only id: {error}")))?;
    if exists.is_none() {
        return Ok(Outcome::Rejected {
            code: ErrorCode::NotFound,
            message: "no such row for this account".to_string(),
        });
    }
    Ok(Outcome::Applied {
        seq: seq_after(conn, user_id, entity, mutation.id).await?,
    })
}

/// `user_settings`: keyed by the caller, and the mutation's id is ignored (A179).
async fn apply_settings(
    conn: &mut PgConnection,
    user_id: Uuid,
    mutation: &Mutation,
) -> AppResult<Outcome> {
    let mut row = row_of(mutation);
    let Some(object) = row.as_object_mut() else {
        return Ok(empty_payload());
    };
    object.insert("user_id".to_string(), json!(user_id));

    let columns = write_columns(Entity::UserSettings, &row);
    let sets = columns
        .iter()
        .filter(|column| **column != "user_id")
        .map(|column| format!("{column} = excluded.{column}"))
        .collect::<Vec<_>>()
        .join(", ");
    if sets.is_empty() {
        return Ok(empty_payload());
    }
    let sql = format!(
        "insert into user_settings ({cols})
         select {cols} from jsonb_populate_record(null::user_settings, $1::jsonb)
         on conflict (user_id) do update set {sets}",
        cols = columns.join(", "),
    );

    let statement = sqlx::query(sqlx::AssertSqlSafe(sql)).bind(&row);
    if let Err(outcome) = as_outcome(run_statement(statement, conn).await)? {
        return Ok(outcome);
    }
    Ok(Outcome::Applied {
        seq: seq_after(conn, user_id, Entity::UserSettings, user_id).await?,
    })
}
