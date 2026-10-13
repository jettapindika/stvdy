//! `POST|GET /workspaces/:id/decks`, `GET /decks/:id`, `GET /decks/:id/due`,
//! `POST /flashcards/:id/review`.
//!
//! The order of operations in `create_deck` is the contract: verified email, owned
//! workspace, owned and ready sources, monthly card limit, credit spend, insert the
//! deck, enqueue the job. Every check that can fail runs before the spend, so a
//! refused request costs nothing.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{delete, get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
// `Row` is required for `row.get::<T, _>("column")`; a trait object's methods resolve
// without an import, but a `PgRow`'s do not.
use sqlx::PgPool;
use sqlx::Row;
use uuid::Uuid;

use stvdy_core::entitlements::Feature;
use stvdy_core::sm2::{CardState, Rating, Review, replay};

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{jobs, workspaces};

/// The action name in `usage_events` and the ledger, so a cost report can separate
/// generation from chat.
pub(crate) const GENERATE_ACTION: &str = "generate_flashcards";

/// The most cards one request may ask for.
///
/// The credit cost is quoted from the requested count, so accepting more than the
/// generator will produce would overcharge. A larger request is refused rather
/// than truncated.
pub const MAX_REQUESTED_CARDS: u32 = 50;

/// How many cards one credit batch covers.
///
/// The price in `config/plans.toml` is per batch (`flashcard_batch_10`), so the
/// quote is `ceil(count / CARDS_PER_BATCH)` batches. It is named rather than
/// written as `10` in two places so the descriptor the SPA renders and the cost
/// the route charges cannot drift.
pub const CARDS_PER_BATCH: i64 = 10;

/// The most sources one deck may draw on. A deck from twenty sources is a topic,
/// not a deck, and the prompt has a chunk budget.
const MAX_SOURCES: usize = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{id}/decks", post(create_deck).get(list_decks))
        .route("/decks/{id}", get(get_deck))
        .route("/decks/{id}/due", get(due_cards))
        .route("/flashcards/{id}/review", post(review_card))
        .route("/flashcard-reviews/{id}", delete(undo_review))
}

/// How long after a review the student may take it back.
///
/// The spec names the window (`SPEC.md:646`, "undo, 5-minute window") and it exists
/// because undo is for a mis-tap, not for a change of mind about the schedule: a
/// review that has stood for longer than this has already informed the day's plan,
/// and deleting it would silently rewrite a schedule the student has seen.
const UNDO_WINDOW_MINUTES: i64 = 5;

/// `DELETE /flashcard-reviews/:id` — undo a grade, inside the window.
///
/// **Undo is a delete plus a replay, not an inverse calculation.** `flashcard_reviews`
/// is append-only and the card's state is a pure function of the rows
/// (`replay(created_at, &reviews)`), so removing the row and replaying the remaining
/// history produces exactly the state the card would have had if the review had never
/// happened. The alternative — subtracting one review's effect — needs a second
/// implementation of SM-2's inverse, and a second implementation of a schedule is how
/// two devices start disagreeing (the same reason the client never computes an
/// interval).
///
/// Three refusals, and each is a fact the student needs rather than a generic 400:
///
///  * **Not this account's review** is a 404, like every other scoped read.
///  * **Outside the window** is a 409: the review exists, and the answer is that it is
///    too late rather than that it is missing.
///  * **Already deleted** is a 404 on the second call, which makes the operation
///    idempotent from the student's side — a retried tap is not an error.
pub async fn undo_review(
    State(state): State<AppState>,
    user: AuthUser,
    Path(review_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let now = state.clock.now();

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning an undo: {error}")))?;

    // The same lock `review_card` takes, and for the same reason: an undo racing a
    // review on the same account would have one replay a history the other is
    // still writing.
    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user.user_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("taking the review lock: {error}")))?;

    let row = sqlx::query_as::<_, (Uuid, DateTime<Utc>)>(
        "select flashcard_id, reviewed_at from flashcard_reviews
          where id = $1 and user_id = $2",
    )
    .bind(review_id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the review: {error}")))?
    .ok_or(AppError::NotFound)?;

    let (card_id, reviewed_at) = row;
    if reviewed_at < now - chrono::Duration::minutes(UNDO_WINDOW_MINUTES) {
        return Err(AppError::Conflict(format!(
            "this review is older than {UNDO_WINDOW_MINUTES} minutes and cannot be undone"
        )));
    }

    sqlx::query("delete from flashcard_reviews where id = $1 and user_id = $2")
        .bind(review_id)
        .bind(user.user_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("deleting the review: {error}")))?;

    // The card's `created_at`, which the replay needs as its starting instant.
    let created_at: DateTime<Utc> = sqlx::query_scalar(
        "select f.created_at from flashcards f
           join decks d on d.id = f.deck_id
          where f.id = $1 and d.user_id = $2",
    )
    .bind(card_id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the card: {error}")))?
    .ok_or(AppError::NotFound)?;

    let remaining = sqlx::query(
        "select id, rating, reviewed_at from flashcard_reviews
          where flashcard_id = $1 order by reviewed_at asc, id asc",
    )
    .bind(card_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the reviews: {error}")))?;

    let mut reviews = Vec::with_capacity(remaining.len());
    for row in &remaining {
        let raw: String = row.get("rating");
        let rating = Rating::parse(&raw).map_err(|_| {
            AppError::Internal(anyhow::anyhow!(
                "stored review rating {raw:?} is not one of again/hard/good/easy"
            ))
        })?;
        reviews.push(Review {
            id: row.get("id"),
            rating,
            reviewed_at: row.get("reviewed_at"),
        });
    }

    let CardState {
        ease,
        interval_days,
        due_at,
    } = replay(created_at, &reviews);

    sqlx::query(
        "update flashcards
            set ease = $2, interval_days = $3, due_at = $4, updated_at = now()
          where id = $1",
    )
    .bind(card_id)
    .bind(ease)
    .bind(interval_days)
    .bind(due_at)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("updating the card: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing an undo: {error}")))?;

    // The card's state after the replay, so the screen renders the schedule the
    // server now holds rather than a locally guessed one.
    Ok(Json(serde_json::json!({
        "undone": true,
        "card": {
            "id": card_id,
            "ease": ease,
            "interval_days": interval_days,
            "due_at": due_at,
        }
    })))
}

#[derive(Debug, Deserialize)]
pub struct CreateDeckBody {
    pub title: String,
    pub source_ids: Vec<Uuid>,
    /// How many cards to ask for. The generator may return fewer; it is told never
    /// to pad.
    pub count: u32,
}

/// The deck as every deck route serialises it.
///
/// `pub(crate)` because the agent's confirm path returns it too: a confirmed `create_flashcards`
/// proposal answers with the same body `POST /decks` answers with (A276), so the card can show
/// what the student just got without a second JSON shape to keep in step.
#[derive(Debug, Serialize)]
pub(crate) struct DeckJson {
    /// The only field the crate reads: the confirm path needs the id to name the row in the
    /// generation job's payload. Everything else is for the client and stays private, so the
    /// JSON shape has exactly one writer.
    pub(crate) id: Uuid,
    workspace_id: Uuid,
    title: String,
    source_ids: Vec<Uuid>,
    status: String,
    error: Option<String>,
    requested_count: i32,
    card_count: i64,
    created_at: DateTime<Utc>,
}

/// A due card as the study queue serialises it.
///
/// `pub(crate)` because [`due_in`] is the one due-cards query and the agent's
/// `get_due_flashcards` tool returns the same rows the screen does (A276). The fields stay
/// private: only this module builds one.
#[derive(Debug, Serialize)]
pub(crate) struct CardJson {
    id: Uuid,
    deck_id: Uuid,
    front: String,
    back: String,
    chunk_id: Option<Uuid>,
    ease: f32,
    interval_days: i32,
    due_at: DateTime<Utc>,
}

/// `POST /workspaces/:id/decks`. 201 with the deck in status `generating`.
pub async fn create_deck(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateDeckBody>,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a deck: {error}")))?;

    let (deck, cost) = create_in(&mut tx, &state, &user, workspace_id, body).await?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing the deck: {error}")))?;

    enqueue_generation(&state, deck.id, user.user_id, cost).await?;

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({ "deck": deck })),
    ))
}

/// Creates the deck row and charges for it, in the caller's transaction.
///
/// **The one implementation of "create a deck".** `POST /workspaces/:id/decks` calls it, and
/// so does `crate::actions::execute` for a confirmed `create_flashcards` proposal (A276), so
/// a confirmed deck costs exactly what the Studio button costs and the two cannot drift
/// apart on validation, on the source-readiness check or on the batch price.
///
/// The count, the charge and the row commit together, because the monthly limit depends on
/// the count: in separate transactions a second request reads a count the first has not
/// committed yet, and two concurrent requests both pass on the last slot. `reserve_in` is
/// that sequence — the per-user advisory lock, the matrix decision against a snapshot read
/// under the lock, the charge, then this closure's row.
///
/// The job's enqueue is deliberately **not** here: it runs after the commit, through
/// [`enqueue_generation`], so both callers queue it the same way and neither can enqueue a
/// job for a row whose transaction rolled back.
pub(crate) async fn create_in(
    conn: &mut sqlx::PgConnection,
    state: &AppState,
    user: &AuthUser,
    workspace_id: Uuid,
    body: CreateDeckBody,
) -> AppResult<(DeckJson, i64)> {
    let title = body.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(AppError::Validation(
            "the deck title must be between 1 and 120 characters".to_string(),
        ));
    }
    if body.count == 0 || body.count > MAX_REQUESTED_CARDS {
        return Err(AppError::Validation(format!(
            "count must be between 1 and {MAX_REQUESTED_CARDS}"
        )));
    }
    if body.source_ids.is_empty() || body.source_ids.len() > MAX_SOURCES {
        return Err(AppError::Validation(format!(
            "a deck is generated from between 1 and {MAX_SOURCES} sources"
        )));
    }

    // Deduplicated before it is validated or stored, because `count(*)` counts rows
    // and `len()` counts the request: a client that sent the same source twice would
    // otherwise be refused with "every source must be one of this workspace's", which
    // is false -- the source is fine, the array had a repeat. First-seen order is
    // kept so the stored `source_ids` reads the way the client sent it.
    let mut source_ids: Vec<Uuid> = Vec::with_capacity(body.source_ids.len());
    for id in &body.source_ids {
        if !source_ids.contains(id) {
            source_ids.push(*id);
        }
    }
    let source_ids = source_ids;

    workspaces::owned_name(state, user.user_id, workspace_id).await?;

    // Every source must be this user's, in this workspace, and finished ingesting:
    // generating from a source whose chunks do not exist yet would produce a deck
    // with no cards and no explanation.
    let ready: i64 = sqlx::query_scalar(
        "select count(*)::bigint from sources
          where id = any($1) and workspace_id = $2 and user_id = $3
            and status = 'ready' and deleted_at is null",
    )
    .bind(&source_ids)
    .bind(workspace_id)
    .bind(user.user_id)
    .fetch_one(&mut *conn)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("checking the deck's sources: {error}")))?;
    if ready != source_ids.len() as i64 {
        return Err(AppError::Validation(
            "every source must be one of this workspace's, and finished indexing".to_string(),
        ));
    }

    // The cost is per ten cards, from the config: `flashcard_batch_10 = 4` is a
    // batch price, and asking for 21 cards is three batches.
    // `i64::div_ceil` is still unstable, and `count` is positive here, so this is
    // an exact ceiling division.
    let batches = (i64::from(body.count) + CARDS_PER_BATCH - 1) / CARDS_PER_BATCH;
    let cost = batches * state.config.plans.credits.flashcard_batch_10;

    // Bound before the closure so `async move` captures references rather than the values
    // the caller still needs below (the deck's JSON body, the job payload).
    let user_id = user.user_id;
    let title_ref = title.as_str();
    let source_ids_ref = source_ids.as_slice();

    let (_, (deck_id, created_at)) = crate::entitlements::reserve_in(
        conn,
        state,
        user,
        Some(Feature::FlashcardBatch { n: body.count }),
        GENERATE_ACTION,
        cost,
        |conn, usage_event_id| {
            Box::pin(async move {
                let row: (Uuid, DateTime<Utc>) = sqlx::query_as(
                    "insert into decks
                        (user_id, workspace_id, title, source_ids, status, requested_count,
                         credits_spent, usage_event_id)
                     values ($1, $2, $3, $4, 'generating', $5, $6, $7)
                     returning id, created_at",
                )
                .bind(user_id)
                .bind(workspace_id)
                .bind(title_ref)
                .bind(source_ids_ref)
                .bind(body.count as i32)
                .bind(cost as i32)
                // The id of the charge this transaction just wrote, passed through the
                // closure: looking up "the newest usage row for this user" instead would
                // attribute the provider's cost to whichever request finished last.
                .bind(usage_event_id)
                .fetch_one(&mut *conn)
                .await
                .map_err(|error| {
                    // A failed insert needs no refund by hand: the charge, the reservation
                    // and the row are all in this transaction, so rolling back unspends
                    // the credit.
                    AppError::Internal(anyhow::anyhow!("creating the deck: {error}"))
                })?;
                Ok(row)
            })
        },
    )
    .await?;

    // Built from `DeckJson`, the same type every other route serialises, so the
    // 201 body cannot drift from `GET /decks/:id`. The hand-built body omitted
    // `created_at`, which the SPA's `Deck` declares: a false type assertion.
    let deck = DeckJson {
        id: deck_id,
        workspace_id,
        title,
        source_ids: source_ids.clone(),
        status: "generating".to_string(),
        error: None,
        requested_count: body.count as i32,
        card_count: 0,
        created_at,
    };

    Ok((deck, cost))
}

/// Queues a deck's generation, refunding the charge if the queue refuses it.
///
/// **The one implementation of "queue a deck's generation".** The route and the agent's
/// confirm path both call it (A276), so the compensation is the same on both: a deck that
/// will never generate must not keep the credits, and a second refund path would be a second
/// answer to what a failed generation refunds.
pub(crate) async fn enqueue_generation(
    state: &AppState,
    deck_id: Uuid,
    user_id: Uuid,
    cost: i64,
) -> AppResult<()> {
    if let Err(error) = jobs::enqueue(
        &state.pool,
        &state.jobs,
        GENERATE_ACTION,
        serde_json::json!({ "deck_id": deck_id }),
        None,
    )
    .await
    {
        crate::jobs::handlers::generate::fail_and_refund(
            state,
            deck_id,
            user_id,
            i32::try_from(cost).unwrap_or(i32::MAX),
            "the generation job could not be queued",
        )
        .await;
        return Err(AppError::Internal(anyhow::anyhow!(
            "enqueueing the generation job: {error}"
        )));
    }
    Ok(())
}

/// `GET /workspaces/:id/decks`, newest first, tombstones excluded.
///
/// Added beyond the spec's route list: without it a deck is unreachable after a
/// page reload, which makes the phase's own exit gate untestable by hand (A81).
pub async fn list_decks(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    workspaces::owned_name(&state, user.user_id, workspace_id).await?;
    let rows = sqlx::query(
        "select d.id, d.workspace_id, d.title, d.source_ids, d.status, d.error,
                d.requested_count, d.created_at,
                (select count(*) from flashcards f
                  where f.deck_id = d.id and f.deleted_at is null) as card_count
           from decks d
          where d.user_id = $1 and d.workspace_id = $2 and d.deleted_at is null
          order by d.created_at desc, d.id",
    )
    .bind(user.user_id)
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing decks: {error}")))?;

    let decks: Vec<DeckJson> = rows.iter().map(deck_from_row).collect();
    // The cost descriptor the SPA's panel renders, from the config rather than a
    // literal: the roadmap forbids a hardcoded limit or cost in the client, so
    // the price and the request cap are served with the decks the panel already
    // fetches. A client that ignores the field still reads `decks`.
    Ok(Json(serde_json::json!({
        "decks": decks,
        "cost": {
            "per_batch": state.config.plans.credits.flashcard_batch_10,
            "batch_size": CARDS_PER_BATCH,
            "max_cards": MAX_REQUESTED_CARDS,
        }
    })))
}

/// `GET /decks/:id`: the deck, its cards, and its generation state.
pub async fn get_deck(
    State(state): State<AppState>,
    user: AuthUser,
    Path(deck_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let deck = owned_deck(&state, user.user_id, deck_id).await?;
    let cards = sqlx::query(
        "select id, deck_id, front, back, chunk_id, ease, interval_days, due_at
           from flashcards
          where deck_id = $1 and deleted_at is null
          order by created_at, id",
    )
    .bind(deck_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the deck's cards: {error}")))?;

    let cards: Vec<CardJson> = cards.iter().map(card_from_row).collect();
    let mut deck = deck;
    deck.card_count = cards.len() as i64;
    Ok(Json(serde_json::json!({ "deck": deck, "cards": cards })))
}

#[derive(Debug, Deserialize)]
pub struct DueQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /decks/:id/due`: the study queue, soonest first.
///
/// Ordered by `(due_at, id)` so a refetch cannot reshuffle the queue under the
/// student's finger, and capped so a deck that has never been studied does not
/// return a thousand cards in one response.
pub async fn due_cards(
    State(state): State<AppState>,
    user: AuthUser,
    Path(deck_id): Path<Uuid>,
    Query(query): Query<DueQuery>,
) -> AppResult<Json<serde_json::Value>> {
    owned_deck(&state, user.user_id, deck_id).await?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let now = state.clock.now();

    let cards = due_in(&state.pool, user.user_id, Some(deck_id), now, limit).await?;
    Ok(Json(serde_json::json!({ "cards": cards, "now": now })))
}

/// The one due-cards query, for one deck or for every deck the caller owns.
///
/// `GET /decks/:id/due` calls it with `Some(deck_id)`, and the agent's
/// `get_due_flashcards` READ tool calls it with the tool's own `deck_id` — which is
/// `Option<Uuid>` because "what is due" without naming a deck is the question a student
/// actually asks (A276). One statement, two callers, so the tool's queue is the screen's
/// queue.
///
/// Ownership is the join through `decks`, not a check the caller remembers: a `deck_id` that
/// is not the caller's returns no rows rather than another account's cards, which is the
/// cross-tenant read `chunks` needed the same join to avoid. The route's `owned_deck` call
/// above is what turns that empty answer into a 404, and the tool does not need a 404 — it
/// needs the truth, which is "no cards".
///
/// Ordered by `(due_at, id)` so a refetch cannot reshuffle the queue under the student's
/// finger.
pub(crate) async fn due_in(
    pool: &PgPool,
    user_id: Uuid,
    deck_id: Option<Uuid>,
    now: DateTime<Utc>,
    limit: i64,
) -> AppResult<Vec<CardJson>> {
    let rows = sqlx::query(
        "select f.id, f.deck_id, f.front, f.back, f.chunk_id, f.ease, f.interval_days, f.due_at
           from flashcards f
           join decks d on d.id = f.deck_id
          where d.user_id = $1 and d.deleted_at is null
            and ($2::uuid is null or f.deck_id = $2)
            and f.deleted_at is null and f.due_at <= $3
          order by f.due_at asc, f.id asc
          limit $4",
    )
    .bind(user_id)
    .bind(deck_id)
    .bind(now)
    .bind(limit)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading due cards: {error}")))?;

    Ok(rows.iter().map(card_from_row).collect())
}

#[derive(Debug, Deserialize)]
pub struct ReviewBody {
    /// Client-generated, so a retry after a lost response cannot apply twice.
    #[serde(default)]
    pub id: Option<Uuid>,
    pub rating: String,
    /// The device's clock. Absent means the server's, which is what an online
    /// review uses.
    #[serde(default)]
    pub reviewed_at: Option<DateTime<Utc>>,
}

/// A review further in the future than this is refused: a device with a wrong
/// clock would otherwise pin a card out of the queue for months.
const FUTURE_REVIEW_TOLERANCE_SECS: i64 = 24 * 60 * 60;

/// `POST /flashcards/:id/review`.
///
/// The whole point of the phase: the review is inserted, the card's state is
/// replayed from *every* review in `(reviewed_at, id)` order, and the result is
/// written back. Two devices therefore converge on one state rather than on
/// whichever wrote last.
pub async fn review_card(
    State(state): State<AppState>,
    user: AuthUser,
    Path(card_id): Path<Uuid>,
    Json(body): Json<ReviewBody>,
) -> AppResult<Json<serde_json::Value>> {
    let rating =
        Rating::parse(&body.rating).map_err(|error| AppError::Validation(error.to_string()))?;
    let now = state.clock.now();
    let reviewed_at = body.reviewed_at.unwrap_or(now);
    if reviewed_at > now + chrono::Duration::seconds(FUTURE_REVIEW_TOLERANCE_SECS) {
        return Err(AppError::Validation(
            "the review's timestamp is in the future; check the device clock".to_string(),
        ));
    }
    let review_id = body.id.unwrap_or_else(Uuid::new_v4);

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a review: {error}")))?;

    // The same advisory lock the spend and the sync triggers take, and the same
    // reason: two devices reviewing one card concurrently would each replay only
    // their own review, and the second `update` would silently drop the other.
    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user.user_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("taking the review lock: {error}")))?;

    let card = sqlx::query(
        "select f.id, f.created_at
           from flashcards f
           join decks d on d.id = f.deck_id
          where f.id = $1 and d.user_id = $2 and f.deleted_at is null and d.deleted_at is null",
    )
    .bind(card_id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the card: {error}")))?
    .ok_or(AppError::NotFound)?;

    let created_at: DateTime<Utc> = card.get("created_at");

    let inserted = sqlx::query(
        "insert into flashcard_reviews (id, user_id, flashcard_id, rating, reviewed_at, device_id)
         values ($1, $2, $3, $4, $5, $6)
         on conflict (id) do nothing",
    )
    .bind(review_id)
    .bind(user.user_id)
    .bind(card_id)
    .bind(rating.as_str())
    .bind(reviewed_at)
    .bind(user.device_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing the review: {error}")))?
    .rows_affected()
        > 0;

    // Replayed from the full history, inside the lock, so the stored state is a
    // function of the rows and not of this request's arrival order.
    let rows = sqlx::query(
        "select id, rating, reviewed_at from flashcard_reviews
          where flashcard_id = $1 order by reviewed_at asc, id asc",
    )
    .bind(card_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the reviews: {error}")))?;

    let mut reviews = Vec::with_capacity(rows.len());
    for row in &rows {
        let raw: String = row.get("rating");
        // A stored rating the enum cannot read is a data defect, not a user error.
        // Skipping it would silently change the schedule, so this fails loudly.
        let rating = Rating::parse(&raw).map_err(|_| {
            AppError::Internal(anyhow::anyhow!(
                "stored review rating {raw:?} is not one of again/hard/good/easy"
            ))
        })?;
        reviews.push(Review {
            id: row.get("id"),
            rating,
            reviewed_at: row.get("reviewed_at"),
        });
    }

    let CardState {
        ease,
        interval_days,
        due_at,
    } = replay(created_at, &reviews);

    sqlx::query(
        "update flashcards
            set ease = $2, interval_days = $3, due_at = $4, updated_at = now()
          where id = $1",
    )
    .bind(card_id)
    .bind(ease)
    .bind(interval_days)
    .bind(due_at)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("updating the card: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a review: {error}")))?;

    Ok(Json(serde_json::json!({
        "applied": inserted,
        "card": {
            "id": card_id,
            "ease": ease,
            "interval_days": interval_days,
            "due_at": due_at,
        }
    })))
}

/// The deck, or 404 when it is not this user's. Tombstones are excluded, so a
/// deleted deck is indistinguishable from one that never existed.
async fn owned_deck(state: &AppState, user_id: Uuid, deck_id: Uuid) -> AppResult<DeckJson> {
    let row = sqlx::query(
        "select d.id, d.workspace_id, d.title, d.source_ids, d.status, d.error,
                d.requested_count, d.created_at,
                (select count(*) from flashcards f
                  where f.deck_id = d.id and f.deleted_at is null) as card_count
           from decks d
          where d.id = $1 and d.user_id = $2 and d.deleted_at is null",
    )
    .bind(deck_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the deck: {error}")))?;

    row.as_ref().map(deck_from_row).ok_or(AppError::NotFound)
}

fn deck_from_row(row: &sqlx::postgres::PgRow) -> DeckJson {
    DeckJson {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        title: row.get("title"),
        source_ids: row.get("source_ids"),
        status: row.get("status"),
        error: row.get("error"),
        requested_count: row.get("requested_count"),
        card_count: row.get("card_count"),
        created_at: row.get("created_at"),
    }
}

fn card_from_row(row: &sqlx::postgres::PgRow) -> CardJson {
    CardJson {
        id: row.get("id"),
        deck_id: row.get("deck_id"),
        front: row.get("front"),
        back: row.get("back"),
        chunk_id: row.get("chunk_id"),
        ease: row.get("ease"),
        interval_days: row.get("interval_days"),
        due_at: row.get("due_at"),
    }
}
