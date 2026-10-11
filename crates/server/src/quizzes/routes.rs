//! `POST|GET /workspaces/:id/quizzes`, `GET /quizzes/:id`,
//! `POST /quizzes/:id/attempts`, `GET /quizzes/:id/results`.
//!
//! The order of operations in `create_quiz` is the contract, and it is the one Phase 4
//! landed on after shipping it wrong: verified email, owned workspace, owned and ready
//! sources, then **one transaction** holding the per-user advisory lock for the monthly
//! count, the charge, and the row that reserves the quiz. Counting outside that
//! transaction reads a count the previous request has not committed yet, and a quiz that
//! is still generating has no `quiz_questions` rows at all, so a burst of requests each
//! sees zero used and every one of them is allowed.

use axum::Json;
use axum::Router;
use axum::extract::{Path, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
// `Row` is required for `row.get::<T, _>("column")`.
use sqlx::Row;
use uuid::Uuid;

use stvdy_core::entitlements::Feature;
use stvdy_core::quiz::{AnswerRecord, QuizSummary, grade_mcq, summarize};

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{jobs, workspaces};

/// The action name in `usage_events` and the ledger, so a cost report can separate quiz
/// generation from deck generation and from chat.
pub(crate) const GENERATE_ACTION: &str = "generate_quiz";

/// The most questions one request may ask for.
///
/// The credit cost is quoted from the requested count, so accepting more than the
/// generator will produce would overcharge. A larger request is refused rather than
/// truncated.
pub const MAX_REQUESTED_QUESTIONS: u32 = 50;

/// How many questions one credit batch covers.
///
/// The price in `config/plans.toml` is per batch (`quiz_10`), so the quote is
/// `ceil(count / QUESTIONS_PER_BATCH)` batches. Named rather than written as `10` in two
/// places so the descriptor the SPA renders and the cost the route charges cannot drift.
pub const QUESTIONS_PER_BATCH: i64 = 10;

/// The most sources one quiz may draw on, for the same reason a deck is bounded: the
/// prompt has a chunk budget.
const MAX_SOURCES: usize = 20;

/// The difficulties the module accepts. Stored as text, so this is the only place the
/// set is enforced.
const DIFFICULTIES: [&str; 3] = ["easy", "medium", "hard"];

/// The question-type settings the module accepts.
const TYPE_SETTINGS: [&str; 3] = ["mcq", "short", "mixed"];

pub fn routes() -> Router<AppState> {
    Router::new()
        .route(
            "/workspaces/{id}/quizzes",
            post(create_quiz).get(list_quizzes),
        )
        .route("/quizzes/{id}", get(get_quiz))
        .route("/quizzes/{id}/attempts", post(record_attempt))
        .route("/quizzes/{id}/results", get(quiz_results))
}

#[derive(Debug, Deserialize)]
pub struct CreateQuizBody {
    pub title: String,
    pub source_ids: Vec<Uuid>,
    /// How many questions to ask for. The generator may return fewer; it is told never
    /// to pad.
    pub count: u32,
    pub difficulty: String,
    /// `mcq`, `short` or `mixed`. Stored, because the job rebuilds the prompt from the
    /// row rather than from this request.
    pub types: String,
}

#[derive(Debug, Serialize)]
struct QuizJson {
    id: Uuid,
    workspace_id: Uuid,
    title: String,
    /// Nullable in the schema (`0010_quizzes_attempts.sql`), and null for a quiz
    /// created before the column carried a value. Decoding it into a `String`
    /// panicked the whole handler on such a row, which is why it is an `Option`
    /// here: a legacy row must render, not take the route down.
    difficulty: Option<String>,
    question_types: String,
    source_ids: Vec<Uuid>,
    status: String,
    error: Option<String>,
    requested_count: i32,
    question_count: i64,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct QuestionJson {
    id: Uuid,
    quiz_id: Uuid,
    kind: String,
    prompt: String,
    options: Option<Vec<String>>,
    answer: String,
    explanation: String,
    key_points: Option<Vec<String>>,
    chunk_id: Option<Uuid>,
    /// What the student already answered, or null. This is what makes the quiz
    /// resumable; the SPA seeds its session from it.
    attempt: Option<AttemptJson>,
}

#[derive(Debug, Serialize)]
struct AttemptJson {
    id: Uuid,
    user_answer: String,
    /// Null means answered but not yet judged, which is not the same as wrong.
    correct: Option<bool>,
    answered_at: DateTime<Utc>,
}

/// `POST /workspaces/:id/quizzes`. 201 with the quiz in status `generating`.
pub async fn create_quiz(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateQuizBody>,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    let title = body.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(AppError::Validation(
            "the quiz title must be between 1 and 120 characters".to_string(),
        ));
    }
    if body.count == 0 || body.count > MAX_REQUESTED_QUESTIONS {
        return Err(AppError::Validation(format!(
            "count must be between 1 and {MAX_REQUESTED_QUESTIONS}"
        )));
    }
    if !DIFFICULTIES.contains(&body.difficulty.as_str()) {
        return Err(AppError::Validation(format!(
            "difficulty must be one of {}",
            DIFFICULTIES.join(", ")
        )));
    }
    if !TYPE_SETTINGS.contains(&body.types.as_str()) {
        return Err(AppError::Validation(format!(
            "types must be one of {}",
            TYPE_SETTINGS.join(", ")
        )));
    }
    if body.source_ids.is_empty() || body.source_ids.len() > MAX_SOURCES {
        return Err(AppError::Validation(format!(
            "a quiz is generated from between 1 and {MAX_SOURCES} sources"
        )));
    }

    // Deduplicated before it is validated or stored, because `count(*)` counts rows and
    // `len()` counts the request: a client that sent the same source twice would
    // otherwise be refused with "every source must be one of this workspace's", which is
    // false. First-seen order is kept so the stored `source_ids` reads the way the client
    // sent it.
    let mut source_ids: Vec<Uuid> = Vec::with_capacity(body.source_ids.len());
    for id in &body.source_ids {
        if !source_ids.contains(id) {
            source_ids.push(*id);
        }
    }
    let source_ids = source_ids;

    workspaces::owned_name(&state, user.user_id, workspace_id).await?;

    // Every source must be this user's, in this workspace, and finished ingesting:
    // generating from a source whose chunks do not exist yet would produce a quiz with
    // no questions and no explanation.
    let ready: i64 = sqlx::query_scalar(
        "select count(*)::bigint from sources
          where id = any($1) and workspace_id = $2 and user_id = $3
            and status = 'ready' and deleted_at is null",
    )
    .bind(&source_ids)
    .bind(workspace_id)
    .bind(user.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("checking the quiz's sources: {error}")))?;
    if ready != source_ids.len() as i64 {
        return Err(AppError::Validation(
            "every source must be one of this workspace's, and finished indexing".to_string(),
        ));
    }

    // The cost is per ten questions, from the config: `quiz_10 = 6` is a batch price, and
    // asking for 21 questions is three batches. `i64::div_ceil` is still unstable, and
    // `count` is positive here, so this is an exact ceiling division.
    let batches = (i64::from(body.count) + QUESTIONS_PER_BATCH - 1) / QUESTIONS_PER_BATCH;
    let cost = batches * state.config.plans.credits.quiz_10;

    // The count, the charge and the reserving row commit together, because the limit
    // depends on the count: in separate transactions a second request reads a count the
    // first has not committed yet, and two concurrent requests both pass on the last slot.
    // `reserve_in` is that sequence — the per-user advisory lock, the matrix decision
    // against a snapshot read under the lock, the charge, then this closure's row.
    //
    // Neither daily cap applies to generation: the plan matrix caps quizzes per **month**
    // (`quizzes_per_month`, the feature below) and has no per-day quiz allowance, so the
    // two slots the chat path uses are both empty inside `reserve_in`.
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a quiz: {error}")))?;

    // Bound before the closure so `async move` captures references rather than the values
    // the handler still needs below (the quiz's JSON body, the job payload).
    let user_id = user.user_id;
    let title_ref = title.as_str();
    let source_ids_ref = source_ids.as_slice();
    let difficulty_ref = body.difficulty.as_str();
    let types_ref = body.types.as_str();

    let (_, (quiz_id, created_at)) = crate::entitlements::reserve_in(
        &mut tx,
        &state,
        &user,
        // One quiz per request whatever it holds (A94); `n` only prices it.
        Some(Feature::CreateQuiz { n: body.count }),
        GENERATE_ACTION,
        cost,
        |conn, usage_event_id| {
            Box::pin(async move {
                // The quiz row *is* the reservation the decision's count read, so it is
                // written in the same transaction as the charge. A failure here rolls the
                // charge back with it, so there is no charge without a quiz and nothing to
                // compensate for by hand.
                let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
                    "insert into quizzes
                        (user_id, workspace_id, title, difficulty, status, source_ids,
                         requested_count, question_types, credits_spent, usage_event_id)
                     values ($1, $2, $3, $4, 'generating', $5, $6, $7, $8, $9)
                     returning id, created_at",
                )
                .bind(user_id)
                .bind(workspace_id)
                .bind(title_ref)
                .bind(difficulty_ref)
                .bind(source_ids_ref)
                .bind(body.count as i32)
                .bind(types_ref)
                .bind(cost as i32)
                .bind(usage_event_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| {
                    AppError::Internal(anyhow::anyhow!("creating the quiz: {error}"))
                })?;
                Ok(row)
            })
        },
    )
    .await?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing the quiz: {error}")))?;

    // The job is enqueued after the commit, so a failure here leaves a quiz the user can
    // see failing rather than a charge with no trace.
    if let Err(error) = jobs::enqueue(
        &state.pool,
        &state.jobs,
        GENERATE_ACTION,
        serde_json::json!({ "quiz_id": quiz_id }),
        None,
    )
    .await
    {
        crate::jobs::handlers::generate_quiz::fail_and_refund(
            &state,
            quiz_id,
            user.user_id,
            cost as i32,
            "the generation job could not be queued",
        )
        .await;
        return Err(AppError::Internal(anyhow::anyhow!(
            "enqueueing the generation job: {error}"
        )));
    }

    // Built from `QuizJson`, the same type every other route serialises, so the 201 body
    // cannot drift from `GET /quizzes/:id`.
    let quiz = QuizJson {
        id: quiz_id,
        workspace_id,
        title,
        // Always set on this path, but the column is nullable so the type is an
        // `Option` (see the struct's doc comment).
        difficulty: Some(body.difficulty),
        question_types: body.types,
        source_ids,
        status: "generating".to_string(),
        error: None,
        requested_count: body.count as i32,
        question_count: 0,
        created_at,
    };

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "quiz": quiz })),
    ))
}

/// `GET /workspaces/:id/quizzes`, newest first, tombstones excluded.
///
/// Added beyond the spec's route list for the same reason the deck list was: without it
/// a quiz is unreachable after a page reload, and the Studio panel cannot poll the ones
/// still generating.
pub async fn list_quizzes(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    workspaces::owned_name(&state, user.user_id, workspace_id).await?;
    let rows = sqlx::query(
        "select q.id, q.workspace_id, q.title, q.difficulty, q.question_types, q.source_ids,
                q.status, q.error, q.requested_count, q.created_at,
                (select count(*) from quiz_questions qq
                  where qq.quiz_id = q.id and qq.deleted_at is null) as question_count
           from quizzes q
          where q.user_id = $1 and q.workspace_id = $2 and q.deleted_at is null
          order by q.created_at desc, q.id",
    )
    .bind(user.user_id)
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing quizzes: {error}")))?;

    let quizzes: Vec<QuizJson> = rows.iter().map(quiz_from_row).collect();
    // The cost descriptor the SPA's panel renders, from the config rather than a literal:
    // the roadmap forbids a hardcoded limit or cost in the client.
    Ok(Json(serde_json::json!({
        "quizzes": quizzes,
        "cost": {
            "per_batch": state.config.plans.credits.quiz_10,
            "batch_size": QUESTIONS_PER_BATCH,
            "max_questions": MAX_REQUESTED_QUESTIONS,
        }
    })))
}

/// `GET /quizzes/:id`: the quiz, its questions, and its generation state.
///
/// The questions carry their answers and explanations: a quiz is not an exam to be
/// proctored, it is a study aid, and the student is meant to see why an answer is right.
pub async fn get_quiz(
    State(state): State<AppState>,
    user: AuthUser,
    Path(quiz_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let quiz = owned_quiz(&state, user.user_id, quiz_id).await?;
    // The student's latest attempt per question comes back with the questions. Without
    // it a reload, or a second device, restarts at question 1 with every answer lost
    // while the attempts sit in the database: an attempt is one of the kinds a client
    // may write offline, so "what did I already answer" has to be readable.
    //
    // A graded attempt supersedes an ungraded one for the same question, and the order
    // is total (grade, then `answered_at`, then id) because two attempts can share an
    // instant: the same rule `quiz_results` uses, so the two views cannot disagree.
    let rows = sqlx::query(
        "select q.id, q.quiz_id, q.kind, q.prompt, q.options, q.answer, q.explanation,
                q.key_points, q.chunk_id,
                a.id as attempt_id, a.user_answer, a.correct, a.answered_at
           from quiz_questions q
           left join lateral (
                select id, user_answer, correct, answered_at
                  from quiz_attempts
                 where question_id = q.id and user_id = $2 and deleted_at is null
                 order by (correct is not null) desc, answered_at desc, id desc
                 limit 1
           ) a on true
          where q.quiz_id = $1 and q.deleted_at is null
          order by q.idx, q.id",
    )
    .bind(quiz_id)
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("loading the quiz's questions: {error}"))
    })?;

    let questions: Vec<QuestionJson> = rows.iter().map(question_from_row).collect();
    let mut quiz = quiz;
    quiz.question_count = questions.len() as i64;
    Ok(Json(
        serde_json::json!({ "quiz": quiz, "questions": questions }),
    ))
}

/// An attempt further in the future than this is refused: a device with a wrong clock
/// would otherwise pin itself to the top of every "latest attempt" read.
const FUTURE_ATTEMPT_TOLERANCE_SECS: i64 = 24 * 60 * 60;

#[derive(Debug, Deserialize)]
pub struct AttemptBody {
    /// Client-generated, so a retry after a lost response cannot apply twice.
    #[serde(default)]
    pub id: Option<Uuid>,
    pub question_id: Uuid,
    pub user_answer: String,
    /// The device's clock. Absent means the server's, which is what an online attempt
    /// uses. An attempt is one of the five kinds a client may write offline, so the
    /// moment the student answered is the client's to report: stamping an offline
    /// answer with the server's sync time would put it in the wrong place in the
    /// student's history, and would make two attempts land in the same instant.
    #[serde(default)]
    pub answered_at: Option<DateTime<Utc>>,
    /// Only read for a short answer. An MCQ is graded by the server, so a client that
    /// claims `correct: true` for a wrong option cannot raise its own score.
    #[serde(default)]
    pub correct: Option<bool>,
}

/// `POST /quizzes/:id/attempts`.
///
/// The attempt is an append-only fact with a client-generated id, which is what makes it
/// safe to send from an offline device: a replay is ignored rather than applied twice,
/// and the response says whether it applied. The score is never written here; it is
/// derived from the attempts when it is read.
pub async fn record_attempt(
    State(state): State<AppState>,
    user: AuthUser,
    Path(quiz_id): Path<Uuid>,
    Json(body): Json<AttemptBody>,
) -> AppResult<Json<serde_json::Value>> {
    owned_quiz(&state, user.user_id, quiz_id).await?;

    let question = sqlx::query(
        "select id, kind, answer from quiz_questions
          where id = $1 and quiz_id = $2 and user_id = $3 and deleted_at is null",
    )
    .bind(body.question_id)
    .bind(quiz_id)
    .bind(user.user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the question: {error}")))?
    .ok_or(AppError::NotFound)?;

    let kind: String = question.get("kind");
    let answer: String = question.get("answer");
    let submitted = body.user_answer.trim().to_string();
    if submitted.is_empty() {
        return Err(AppError::Validation(
            "an attempt must carry an answer".to_string(),
        ));
    }

    // An MCQ is graded here, from the stored answer. A short answer is the student's to
    // mark: no string comparison can judge free prose, so `correct` is taken as given and
    // may be absent, which stores `NULL` and means "answered, not yet judged".
    let correct = if kind == stvdy_core::generation::quiz::MCQ {
        Some(grade_mcq(&answer, &submitted))
    } else {
        body.correct
    };

    let now = state.clock.now();
    let answered_at = body.answered_at.unwrap_or(now);
    // The same bound a review uses, and for the same reason: a device with a wrong
    // clock would otherwise stamp an answer years ahead and pin it to the top of every
    // "latest attempt" read.
    if answered_at > now + chrono::Duration::seconds(FUTURE_ATTEMPT_TOLERANCE_SECS) {
        return Err(AppError::Validation(
            "the attempt's timestamp is in the future; check the device clock".to_string(),
        ));
    }

    let attempt_id = body.id.unwrap_or_else(Uuid::new_v4);
    let applied = sqlx::query(
        "insert into quiz_attempts
            (id, user_id, quiz_id, question_id, user_answer, correct, answered_at, device_id)
         values ($1, $2, $3, $4, $5, $6, $7, $8)
         on conflict (id) do nothing",
    )
    .bind(attempt_id)
    .bind(user.user_id)
    .bind(quiz_id)
    .bind(body.question_id)
    .bind(&submitted)
    .bind(correct)
    .bind(answered_at)
    .bind(user.device_id)
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing the attempt: {error}")))?
    .rows_affected()
        > 0;

    Ok(Json(serde_json::json!({
        "applied": applied,
        "attempt": {
            "id": attempt_id,
            "question_id": body.question_id,
            "correct": correct,
            "answered_at": answered_at,
        }
    })))
}

/// `GET /quizzes/:id/results`: the score and the weak-topic summary.
///
/// Both are computed here, from the stored rows, every time. Nothing about the score is
/// stored, so two devices cannot disagree about it and a replayed attempt cannot
/// double-count.
pub async fn quiz_results(
    State(state): State<AppState>,
    user: AuthUser,
    Path(quiz_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    owned_quiz(&state, user.user_id, quiz_id).await?;

    // One row per question, with its place in the document (the chunk's source and page)
    // and the student's latest attempt, if any. The joins are `left` because a chunk can
    // have been deleted since generation (`chunk_id` is `set null` on delete), and a
    // question whose label never resolved has no chunk at all.
    let rows = sqlx::query(
        "select q.id as question_id, q.kind, q.prompt, q.answer, q.explanation,
                q.options, q.key_points, q.chunk_id,
                c.source_id, c.page, s.title as source_title,
                a.id as attempt_id, a.user_answer, a.correct, a.answered_at
           from quiz_questions q
           left join chunks c on c.id = q.chunk_id
           left join sources s on s.id = c.source_id
           left join lateral (
                select id, user_answer, correct, answered_at
                  from quiz_attempts
                 where question_id = q.id and user_id = $2 and deleted_at is null
                 -- A graded attempt wins over an ungraded one for the same question.
                 -- Marking your own short answer is the fact that matters, and the two
                 -- rows can share an `answered_at` to the microsecond (a client that
                 -- submits and then marks, or a pinned clock in a test), so ordering by
                 -- time alone would make the result depend on a random uuid.
                 order by (correct is not null) desc, answered_at desc, id desc
                 limit 1
           ) a on true
          where q.quiz_id = $1 and q.deleted_at is null
          order by q.idx, q.id",
    )
    .bind(quiz_id)
    .bind(user.user_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the quiz's results: {error}")))?;

    let mut records = Vec::with_capacity(rows.len());
    let mut detail = Vec::with_capacity(rows.len());
    for row in &rows {
        let attempted: Option<Uuid> = row.get("attempt_id");
        let correct: Option<bool> = row.get("correct");
        let source_id: Option<Uuid> = row.get("source_id");
        let page: Option<i32> = row.get("page");
        let source_title: Option<String> = row.get("source_title");
        let kind: String = row.get("kind");

        records.push(AnswerRecord {
            kind: kind.clone(),
            source_id,
            source_title,
            page,
            attempted: attempted.is_some(),
            correct,
        });

        detail.push(serde_json::json!({
            "question_id": row.get::<Uuid, _>("question_id"),
            "kind": kind,
            "prompt": row.get::<String, _>("prompt"),
            "answer": row.get::<String, _>("answer"),
            "explanation": row.get::<String, _>("explanation"),
            "chunk_id": row.get::<Option<Uuid>, _>("chunk_id"),
            "user_answer": row.get::<Option<String>, _>("user_answer"),
            "correct": correct,
            "answered_at": row.get::<Option<DateTime<Utc>>, _>("answered_at"),
        }));
    }

    let QuizSummary {
        total,
        answered,
        graded,
        correct,
        score,
        topics,
    } = summarize(&records);

    Ok(Json(serde_json::json!({
        "summary": {
            "total": total,
            "answered": answered,
            "graded": graded,
            "correct": correct,
            "score": score,
            "topics": topics.iter().map(|topic| serde_json::json!({
                "source_id": topic.source_id,
                "source_title": topic.source_title,
                "page": topic.page,
                "answered": topic.answered,
                "graded": topic.graded,
                "correct": topic.correct,
            })).collect::<Vec<_>>(),
        },
        "questions": detail,
    })))
}

/// The quiz, or 404 when it is not this user's. Tombstones are excluded, so a deleted
/// quiz is indistinguishable from one that never existed: the id must not confirm that
/// another account's quiz exists.
async fn owned_quiz(state: &AppState, user_id: Uuid, quiz_id: Uuid) -> AppResult<QuizJson> {
    let row = sqlx::query(
        "select q.id, q.workspace_id, q.title, q.difficulty, q.question_types, q.source_ids,
                q.status, q.error, q.requested_count, q.created_at,
                (select count(*) from quiz_questions qq
                  where qq.quiz_id = q.id and qq.deleted_at is null) as question_count
           from quizzes q
          where q.id = $1 and q.user_id = $2 and q.deleted_at is null",
    )
    .bind(quiz_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the quiz: {error}")))?;

    row.as_ref().map(quiz_from_row).ok_or(AppError::NotFound)
}

fn quiz_from_row(row: &sqlx::postgres::PgRow) -> QuizJson {
    QuizJson {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        title: row.get("title"),
        difficulty: row.get("difficulty"),
        question_types: row.get("question_types"),
        source_ids: row.get("source_ids"),
        status: row.get("status"),
        error: row.get("error"),
        requested_count: row.get("requested_count"),
        question_count: row.get("question_count"),
        created_at: row.get("created_at"),
    }
}

fn question_from_row(row: &sqlx::postgres::PgRow) -> QuestionJson {
    let options: Option<serde_json::Value> = row.get("options");
    let key_points: Option<serde_json::Value> = row.get("key_points");
    QuestionJson {
        id: row.get("id"),
        quiz_id: row.get("quiz_id"),
        kind: row.get("kind"),
        prompt: row.get("prompt"),
        // Stored as `jsonb`, so it comes back as JSON rather than as a `Vec<String>`.
        // A malformed value would be a data defect; `from_value` returning `None` here
        // leaves the field null rather than failing the whole quiz read.
        options: options.and_then(|value| serde_json::from_value(value).ok()),
        answer: row.get("answer"),
        explanation: row.get("explanation"),
        key_points: key_points.and_then(|value| serde_json::from_value(value).ok()),
        chunk_id: row.get("chunk_id"),
        // `Option` from the left join: a question nobody has answered has no attempt,
        // which is a different state from an attempt with a null verdict.
        attempt: row
            .get::<Option<Uuid>, _>("attempt_id")
            .map(|id| AttemptJson {
                id,
                user_answer: row.get("user_answer"),
                correct: row.get("correct"),
                answered_at: row.get("answered_at"),
            }),
    }
}
