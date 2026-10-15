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
/// The guard is `credits_spent > 0`, consumed by the claim that zeroes it: a
/// second call (a retried job, the enqueue-failure path in the route, or the boot
/// sweep) refunds nothing because there is nothing left to refund.
///
/// The deck is left terminal in every case. When the refund insert cannot be
/// written — an infrastructure failure, not a business one — the charge is kept
/// (`credits_spent > 0`, "charged and not delivered, refund pending") and the
/// status change still commits, because this is only ever called on the final
/// attempt: a rollback would leave the deck `generating` with no retry left to
/// resolve it. `credits::refund_pending_decks` recovers the charge at the next
/// boot.
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

    if claimed.is_some() {
        // The ledger row is issued inside the same transaction as the claim that
        // zeroed `credits_spent`, so a crash between the two cannot charge a user
        // for a deck they never received with no retry left to fix it. The refund
        // runs under a savepoint because Postgres aborts a transaction on any
        // failed statement: without it the recovery below could not run, and the
        // only way to close the transaction would be a rollback that also undid
        // the status change.
        if let Err(error) = sqlx::query("savepoint refund").execute(&mut *tx).await {
            tracing::error!(%error, %deck_id, "could not open the refund savepoint");
            return;
        }

        if let Err(error) =
            crate::credits::refund_in(&mut tx, user_id, i64::from(credits), deck_id, "refund").await
        {
            // A refund that cannot be written is an infrastructure failure, and the
            // job that reached here has no retry left (`run` calls this only on the
            // final attempt). Rolling the whole transaction back would leave the
            // deck `generating` forever, charged, and polled by the client without
            // end, so the status change is committed with `credits_spent` restored
            // to the charged amount: the deck is terminal, and `refund_pending_decks`
            // recovers the charge at the next boot.
            tracing::error!(%error, %deck_id, "could not refund a failed generation; the charge stays pending for recovery");

            // Undo only the failed insert attempt. The claim that marked the deck
            // failed stands, and `credits_spent` is put back explicitly rather than
            // by rolling the claim back: `credits_spent > 0` is then exactly
            // "charged and not delivered, refund pending", which is what the boot
            // sweep looks for.
            if let Err(rollback_error) = sqlx::query("rollback to savepoint refund")
                .execute(&mut *tx)
                .await
            {
                tracing::error!(%rollback_error, %deck_id, "could not roll back the failed refund insert");
                return;
            }
            if let Err(restore_error) =
                sqlx::query("update decks set credits_spent = $2 where id = $1")
                    .bind(deck_id)
                    .bind(credits)
                    .execute(&mut *tx)
                    .await
            {
                tracing::error!(%restore_error, %deck_id, "could not restore the pending charge");
                return;
            }
            if let Err(commit_error) = tx.commit().await {
                tracing::error!(%commit_error, %deck_id, "could not commit the deck failure");
                return;
            }
            return;
        }
    }

    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %deck_id, "could not commit the deck failure");
        return;
    }

    if claimed.is_some() {
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
    //
    // Zeroing `credits_spent` here is what makes `credits_spent > 0` mean "charged
    // and not yet delivered" for the refund guard. Without it a deck that was paid
    // for and delivered still carries its charge, so the loser of a claim race — a
    // second worker that loaded the deck while it was `generating`, called the
    // model, and then failed for its own reason — would satisfy the guard on the
    // winner's charge and flip a `ready`, card-bearing deck to `failed` and refund
    // a generation the user received.
    let claimed: Option<Uuid> = sqlx::query_scalar(
        "update decks
            set status = 'ready', error = null, credits_spent = 0, updated_at = now()
          where id = $1 and status = 'generating' and deleted_at is null
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
            "insert into flashcards (user_id, deck_id, front, back, chunk_id, due_at, created_at)
             values ($1, $2, $3, $4, $5, $6, $6)",
        )
        .bind(deck.user_id)
        .bind(deck.id)
        .bind(&card.front)
        .bind(&card.back)
        .bind(card.chunk_id)
        // The injected clock, not the database's `now()`: a fresh card must be due
        // at the instant the application thinks it is, and a test can pin it.
        //
        // **`created_at` is bound to the same instant, and that is load-bearing.**
        // `replay(created_at, &reviews)` derives the state of a card with *no*
        // reviews from `created_at`, so a card whose `created_at` came from the
        // database clock while its `due_at` came from the injected one has two
        // different "start" instants — and replaying an emptied history would move
        // its due date. That is reachable: undoing a card's only review replays an
        // empty list, and it must land on exactly the state the card was created in.
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
