//! `generate_flashcards`: the job shell around `flashcards::generate`.
//!
//! It owns the transaction boundary (cards plus the deck's status change), the
//! permanent-failure path (status, readable error, refund), and the give-up case:
//! a transient failure on the last allowed attempt is final, and the worker's
//! `mark_failed` is the only place that knows it, so the check is here.

use chrono::{DateTime, Utc};
use sqlx::Row;
use uuid::Uuid;

use crate::flashcards::generate::{self, DeckRow, Generation};
use crate::jobs::{self, ClaimedJob, Failure};
use crate::state::AppState;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let deck_id: Uuid = job
        .payload
        .get("deck_id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid deck_id"))?;

    let deck = match load_deck(state, deck_id).await? {
        Some(deck) => deck,
        // Deleted between enqueue and claim: nothing to do, and not an error.
        None => return Ok(()),
    };

    // Idempotent: a retried job whose first attempt already wrote the cards must
    // not write a second deck's worth, and a deck the enqueue path already failed
    // must not be resurrected into `ready`. The status is the marker.
    if deck.status != "generating" {
        return Ok(());
    }

    match generate::generate(state, &deck).await {
        Ok(generation) => {
            // Read once, here, so every card of one deck shares one `due_at` and it
            // comes from the injected clock rather than the database's.
            let now = state.clock.now();
            insert_cards(state, &deck, generation, now).await?;
            Ok(())
        }
        Err(failure) => {
            // A transient failure on the last attempt is final in every way that
            // matters to the user: no retry is coming, so the deck must stop
            // saying "generating" and the credits must come back.
            let final_attempt = !failure.retryable || job.attempts >= jobs::MAX_ATTEMPTS;
            if final_attempt {
                fail_and_refund(
                    state,
                    deck.id,
                    deck.user_id,
                    deck.credits_spent,
                    &failure.message,
                )
                .await;
            }
            Err(failure)
        }
    }
}

/// The deck a job runs for, joined to its workspace so the prompt can name it.
async fn load_deck(state: &AppState, deck_id: Uuid) -> Result<Option<DeckRow>, Failure> {
    let row = sqlx::query(
        "select d.id, d.user_id, d.requested_count, d.status, d.credits_spent,
                d.usage_event_id, d.source_ids, d.created_at,
                w.name as workspace_name
           from decks d
           join workspaces w on w.id = d.workspace_id
          where d.id = $1 and d.deleted_at is null",
    )
    .bind(deck_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| Failure::transient(format!("loading the deck: {error}")))?;

    Ok(row.as_ref().map(|row| DeckRow {
        id: row.get("id"),
        user_id: row.get("user_id"),
        requested_count: row.get("requested_count"),
        status: row.get("status"),
        credits_spent: row.get("credits_spent"),
        usage_event_id: row.get("usage_event_id"),
        source_ids: row.get("source_ids"),
        workspace_name: row.get("workspace_name"),
        created_at: row.get("created_at"),
    }))
}

/// Marks the deck failed and gives back exactly what was charged, once.
///
/// The guard is the `credits_spent` zeroing in the same statement: a second call
/// (a retried job, or the enqueue-failure path in the route) refunds nothing
/// because there is nothing left to refund.
pub async fn fail_and_refund(
    state: &AppState,
    deck_id: Uuid,
    user_id: Uuid,
    credits: i32,
    message: &str,
) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %deck_id, "could not begin the deck failure transaction");
            return;
        }
    };

    let claimed: Option<i32> = sqlx::query_scalar(
        "update decks set status = 'failed', error = $2, credits_spent = 0, updated_at = now()
          where id = $1 and credits_spent > 0
         returning credits_spent",
    )
    .bind(deck_id)
    .bind(message)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    if claimed.is_none() {
        // Either the deck was already failed, or it never had a charge. Make sure
        // the status is right either way, then stop.
        if let Err(error) = sqlx::query(
            "update decks set status = 'failed', error = $2, updated_at = now()
              where id = $1 and status = 'generating'",
        )
        .bind(deck_id)
        .bind(message)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%error, %deck_id, "could not mark the deck failed");
        }
    }

    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %deck_id, "could not commit the deck failure");
        return;
    }

    if claimed.is_some() {
        if let Err(error) =
            crate::credits::refund(&state.pool, user_id, i64::from(credits), deck_id, "refund")
                .await
        {
            tracing::error!(%error, %deck_id, "could not refund a failed generation");
        }
        tracing::warn!(%deck_id, credits, "generation failed permanently; credits refunded");
    }
}

/// Writes the cards, attaches the provider usage to the usage row, and flips the
/// status, in one transaction.
async fn insert_cards(
    state: &AppState,
    deck: &DeckRow,
    generation: Generation,
    now: DateTime<Utc>,
) -> Result<(), Failure> {
    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| Failure::transient(format!("beginning the insert: {error}")))?;

    // The status flip is the claim: it is the only thing that decides whether this
    // job writes cards, so two claims for one deck cannot both write. The predicate
    // is what makes it a claim rather than an assignment.
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "update decks set status = 'ready', error = null, updated_at = now()
          where id = $1 and status = 'generating'
         returning id",
    )
    .bind(deck.id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| Failure::transient(format!("claiming the deck: {error}")))?;

    if claimed.is_none() {
        // Someone else already finished or failed this deck. Nothing to write, and
        // not an error: the work is done, just not by this attempt. Returning drops
        // the transaction, which rolls it back, and that is the correct outcome
        // since the claim was not ours.
        return Ok(());
    }

    for card in &generation.cards {
        sqlx::query(
            "insert into flashcards (user_id, deck_id, front, back, chunk_id, due_at)
             values ($1, $2, $3, $4, $5, $6)",
        )
        .bind(deck.user_id)
        .bind(deck.id)
        .bind(&card.front)
        .bind(&card.back)
        .bind(card.chunk_id)
        // The injected clock, not the database's `now()`: a fresh card must be due
        // at the instant the application thinks it is, and a test can pin it.
        .bind(now)
        .execute(&mut *tx)
        .await
        .map_err(|error| Failure::transient(format!("inserting a card: {error}")))?;
    }

    tx.commit()
        .await
        .map_err(|error| Failure::transient(format!("committing the deck: {error}")))?;

    if let Some(usage_event_id) = generate::find_usage_event_id(state, deck).await {
        // Attached after the cards exist, because the token counts and the cost only
        // exist once the provider has answered, and the row was created before that
        // so the credit is spent whether or not the model replies.
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
            // failure: the cards are stored and paid for, so the job is not failed.
            tracing::error!(%error, %usage_event_id, "could not attach provider usage");
        }
    }

    if !generation.dropped_labels.is_empty() {
        tracing::warn!(
            deck_id = %deck.id,
            dropped = ?generation.dropped_labels,
            "the model cited labels that were not in the context"
        );
    }

    Ok(())
}
