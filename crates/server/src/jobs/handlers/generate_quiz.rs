//! `generate_quiz`: the job shell around `quizzes::generate`.
//!
//! Mirrors `jobs::handlers::generate` for the deck. It owns the transaction boundary
//! (questions plus the quiz's status change), the permanent-failure path (status,
//! readable error, refund), and the give-up case: a transient failure on the last
//! allowed attempt is final, and the worker's `mark_failed` is the only place that knows
//! it, so the check is here.

use sqlx::Row;
use uuid::Uuid;

use crate::jobs::{self, ClaimedJob, Failure};
use crate::quizzes::generate::{self, Generation, QuizRow};
use crate::state::AppState;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let quiz_id: Uuid = job
        .payload
        .get("quiz_id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid quiz_id"))?;

    let quiz = match load_quiz(state, quiz_id).await? {
        Some(quiz) => quiz,
        // Deleted between enqueue and claim: nothing to do, and not an error.
        None => return Ok(()),
    };

    // Idempotent: a retried job whose first attempt already wrote the questions must not
    // write a second quiz's worth, and a quiz the enqueue path already failed must not be
    // resurrected into `ready`. The status is the marker.
    if quiz.status != "generating" {
        return Ok(());
    }

    match generate::generate(state, &quiz).await {
        Ok(generation) => {
            insert_questions(state, &quiz, generation).await?;
            Ok(())
        }
        Err(failure) => {
            // A transient failure on the last attempt is final in every way that matters
            // to the user: no retry is coming, so the quiz must stop saying "generating"
            // and the credits must come back.
            let final_attempt = !failure.retryable || job.attempts >= jobs::MAX_ATTEMPTS;
            if final_attempt {
                fail_and_refund(
                    state,
                    quiz.id,
                    quiz.user_id,
                    quiz.credits_spent,
                    &failure.message,
                )
                .await;
            }
            Err(failure)
        }
    }
}

/// The quiz a job runs for, joined to its workspace so the prompt can name it.
async fn load_quiz(state: &AppState, quiz_id: Uuid) -> Result<Option<QuizRow>, Failure> {
    let row = sqlx::query(
        "select q.id, q.user_id, q.requested_count, q.difficulty, q.question_types, q.status,
                q.credits_spent, q.usage_event_id, q.source_ids, q.created_at,
                w.name as workspace_name
           from quizzes q
           join workspaces w on w.id = q.workspace_id
          where q.id = $1 and q.deleted_at is null",
    )
    .bind(quiz_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the quiz: {error}")))?;

    Ok(row.as_ref().map(|row| QuizRow {
        id: row.get("id"),
        user_id: row.get("user_id"),
        requested_count: row.get("requested_count"),
        // `difficulty` is nullable in the schema, and a row written by a path that does
        // not generate has no difficulty. Falling back to the middle of the range is
        // better than failing a generation the user paid for, and the route always
        // writes one.
        difficulty: row
            .get::<Option<String>, _>("difficulty")
            .unwrap_or_else(|| "medium".to_string()),
        question_types: row.get("question_types"),
        status: row.get("status"),
        credits_spent: row.get("credits_spent"),
        usage_event_id: row.get("usage_event_id"),
        source_ids: row.get("source_ids"),
        workspace_name: row.get("workspace_name"),
        created_at: row.get("created_at"),
    }))
}

/// Marks the quiz failed and gives back exactly what was charged, once.
///
/// The guard is `credits_spent > 0`, consumed by the claim that zeroes it: a second call
/// (a retried job, the enqueue-failure path in the route, or the boot sweep) refunds
/// nothing because there is nothing left to refund.
///
/// The quiz is left terminal in every case. When the refund insert cannot be written, an
/// infrastructure failure rather than a business one, the charge is kept
/// (`credits_spent > 0`, "charged and not delivered, refund pending") and the status
/// change still commits, because this is only ever called on the final attempt: a
/// rollback would leave the quiz `generating` with no retry left to resolve it.
/// `credits::refund_pending(RefundTarget::Quiz)` recovers the charge at the next boot.
pub async fn fail_and_refund(
    state: &AppState,
    quiz_id: Uuid,
    user_id: Uuid,
    credits: i32,
    message: &str,
) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %quiz_id, "could not begin the quiz failure transaction");
            return;
        }
    };

    let claimed: Option<i32> = sqlx::query_scalar(
        "update quizzes set status = 'failed', error = $2, credits_spent = 0, updated_at = now()
          where id = $1 and credits_spent > 0
         returning credits_spent",
    )
    .bind(quiz_id)
    .bind(message)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    if claimed.is_none() {
        // Either the quiz was already failed, or it never had a charge. Make sure the
        // status is right either way, then stop.
        if let Err(error) = sqlx::query(
            "update quizzes set status = 'failed', error = $2, updated_at = now()
              where id = $1 and status = 'generating'",
        )
        .bind(quiz_id)
        .bind(message)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%error, %quiz_id, "could not mark the quiz failed");
        }
    }

    if claimed.is_some() {
        // The ledger row is issued inside the same transaction as the claim that zeroed
        // `credits_spent`, so a crash between the two cannot charge a user for a quiz
        // they never received with no retry left to fix it. The refund runs under a
        // savepoint because Postgres aborts a transaction on any failed statement:
        // without it the recovery below could not run, and the only way to close the
        // transaction would be a rollback that also undid the status change.
        if let Err(error) = sqlx::query("savepoint refund").execute(&mut *tx).await {
            tracing::error!(%error, %quiz_id, "could not open the refund savepoint");
            return;
        }

        if let Err(error) =
            crate::credits::refund_in(&mut tx, user_id, i64::from(credits), quiz_id, "refund").await
        {
            // A refund that cannot be written is an infrastructure failure, and the job
            // that reached here has no retry left. Rolling the whole transaction back
            // would leave the quiz `generating` forever, charged, and polled by the
            // client without end, so the status change is committed with `credits_spent`
            // restored to the charged amount: the quiz is terminal, and the boot sweep
            // recovers the charge.
            tracing::error!(%error, %quiz_id, "could not refund a failed generation; the charge stays pending for recovery");

            // Undo only the failed insert attempt. The claim that marked the quiz failed
            // stands, and `credits_spent` is put back explicitly rather than by rolling
            // the claim back: `credits_spent > 0` is then exactly "charged and not
            // delivered, refund pending", which is what the boot sweep looks for.
            if let Err(rollback_error) = sqlx::query("rollback to savepoint refund")
                .execute(&mut *tx)
                .await
            {
                tracing::error!(%rollback_error, %quiz_id, "could not roll back the failed refund insert");
                return;
            }
            if let Err(restore_error) =
                sqlx::query("update quizzes set credits_spent = $2 where id = $1")
                    .bind(quiz_id)
                    .bind(credits)
                    .execute(&mut *tx)
                    .await
            {
                tracing::error!(%restore_error, %quiz_id, "could not restore the pending charge");
                return;
            }
            if let Err(commit_error) = tx.commit().await {
                tracing::error!(%commit_error, %quiz_id, "could not commit the quiz failure");
                return;
            }
            return;
        }
    }

    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %quiz_id, "could not commit the quiz failure");
        return;
    }

    if claimed.is_some() {
        tracing::warn!(%quiz_id, credits, "generation failed permanently; credits refunded");
    }
}

/// Writes the questions, attaches the provider usage to the usage row, and flips the
/// status, in one transaction.
async fn insert_questions(
    state: &AppState,
    quiz: &QuizRow,
    generation: Generation,
) -> Result<(), Failure> {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| Failure::transient(format!("beginning the insert: {error}")))?;

    // The status flip is the claim: it is the only thing that decides whether this job
    // writes questions, so two claims for one quiz cannot both write. The predicate is
    // what makes it a claim rather than an assignment.
    //
    // Zeroing `credits_spent` here is what makes `credits_spent > 0` mean "charged and
    // not yet delivered" for the refund guard: without it a quiz that was paid for and
    // delivered still carries its charge, and the loser of a claim race would satisfy the
    // guard on the winner's charge and refund a generation the user received.
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "update quizzes
            set status = 'ready', error = null, credits_spent = 0, updated_at = now()
          where id = $1 and status = 'generating'
         returning id",
    )
    .bind(quiz.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| Failure::transient(format!("claiming the quiz: {error}")))?;

    if claimed.is_none() {
        // Someone else already finished or failed this quiz. Nothing to write, and not
        // an error: the work is done, just not by this attempt. Returning drops the
        // transaction, which rolls it back, and that is the correct outcome since the
        // claim was not ours.
        return Ok(());
    }

    for (index, question) in generation.questions.iter().enumerate() {
        // `options` and `key_points` are `jsonb`, so they are bound as JSON rather than
        // as an array: a short answer stores an explicit `null`, which is what the
        // contract and the client both expect for a kind that has none.
        let options = question
            .options
            .as_ref()
            .map(|options| serde_json::json!(options));
        let key_points = question
            .key_points
            .as_ref()
            .map(|points| serde_json::json!(points));

        sqlx::query(
            "insert into quiz_questions
                (user_id, quiz_id, kind, prompt, options, answer, explanation, key_points,
                 chunk_id, idx)
             values ($1, $2, $3, $4, $5, $6, $7, $8, $9, $10)",
        )
        .bind(quiz.user_id)
        .bind(quiz.id)
        .bind(&question.kind)
        .bind(&question.prompt)
        .bind(options)
        .bind(&question.answer)
        .bind(&question.explanation)
        .bind(key_points)
        .bind(question.chunk_id)
        // The model's order, so the quiz reads the way it was generated.
        .bind(index as i32)
        .execute(&mut *tx)
        .await
        .map_err(|error| Failure::transient(format!("inserting a question: {error}")))?;
    }

    tx.commit()
        .await
        .map_err(|error| Failure::transient(format!("committing the quiz: {error}")))?;

    if let Some(usage_event_id) = generate::find_usage_event_id(state, quiz).await {
        // Attached after the questions exist, because the token counts and the cost only
        // exist once the provider has answered, and the row was created before that so
        // the credit is spent whether or not the model replies.
        if let Err(error) = crate::credits::record_usage(
            &state.pool,
            Some(usage_event_id),
            Some(generation.in_tokens),
            Some(generation.out_tokens),
            generation.provider_cost_usd,
            &generation.prompt_version,
        )
        .await
        {
            // A usage row that loses its cost is a reporting gap, not a user-visible
            // failure: the questions are stored and paid for, so the job is not failed.
            tracing::error!(%error, %usage_event_id, "could not attach provider usage");
        }
    }

    if !generation.dropped_labels.is_empty() {
        tracing::warn!(
            quiz_id = %quiz.id,
            dropped = ?generation.dropped_labels,
            "the model cited labels that were not in the context"
        );
    }

    Ok(())
}
