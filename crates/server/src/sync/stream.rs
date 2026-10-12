//! `GET /sync/stream`: the realtime hint.
//!
//! A per-connection poll of the caller's own `max(seq)`, not `LISTEN/NOTIFY` and not a
//! cross-instance bus: a poll is what stays correct when more than one server instance is
//! serving the same account, and it needs no extra infrastructure (A181). It sends the
//! current seq immediately on connect, so a device that reconnects after a gap pulls at
//! once instead of waiting for the next change.

use std::convert::Infallible;
use std::time::Duration;

use axum::extract::State;
use axum::response::Sse;
use axum::response::sse::Event;
use futures::Stream;
use sqlx::PgPool;
use uuid::Uuid;

use crate::auth::extract::AuthUser;
use crate::chat::sse::HEARTBEAT;
use crate::error::AppResult;
use crate::state::AppState;

/// How often the connection re-reads the caller's `max(seq)`.
pub const POLL_INTERVAL: Duration = Duration::from_secs(2);

/// The one event this stream carries: `event: seq`, `data: {"seq": N}`.
///
/// The name is additive to the roadmap's §4 list, which covers the chat stream;
/// `docs/SYNC.md` and `docs/API.md` record it.
pub fn seq_event(seq: i64) -> Event {
    Event::default()
        .event("seq")
        .json_data(serde_json::json!({ "seq": seq }))
        // `json_data` on a `Value` cannot fail; the fallback exists so this never needs an
        // unwrap in a stream that has no way to report an error.
        .unwrap_or_else(|_| Event::default().event("seq").data(r#"{"seq":0}"#))
}

/// `GET /sync/stream`.
pub async fn stream(
    State(state): State<AppState>,
    user: AuthUser,
) -> AppResult<Sse<impl Stream<Item = Result<Event, Infallible>>>> {
    let pool = state.pool.clone();
    let user_id = user.user_id;
    tracing::info!(%user_id, "sync stream opened");

    let events = async_stream::stream! {
        // The hint's first frame is the caller's clock right now, so a device that
        // reconnects pulls at once rather than waiting for the next change.
        let mut last = match max_seq(&pool, user_id).await {
            Ok(seq) => seq,
            Err(error) => {
                tracing::warn!(%user_id, %error, "the sync stream could not read the clock");
                0
            }
        };
        yield Ok(seq_event(last));

        let mut heartbeat = tokio::time::interval(HEARTBEAT);
        // `interval` fires immediately; the first heartbeat should come a full period in.
        heartbeat.tick().await;

        loop {
            tokio::select! {
                () = tokio::time::sleep(POLL_INTERVAL) => {
                    match max_seq(&pool, user_id).await {
                        Ok(seq) if seq > last => {
                            last = seq;
                            yield Ok(seq_event(seq));
                        }
                        Ok(_) => {}
                        Err(error) => {
                            tracing::warn!(%user_id, %error, "the sync stream could not read the clock");
                        }
                    }
                }
                _ = heartbeat.tick() => {
                    yield Ok(crate::chat::sse::heartbeat());
                }
            }
        }
    };

    Ok(Sse::new(events))
}

/// The caller's own `max(seq)`, scoped by `user_id` in the statement.
async fn max_seq(pool: &PgPool, user_id: Uuid) -> sqlx::Result<i64> {
    sqlx::query_scalar("select coalesce(max(seq), 0) from change_log where user_id = $1")
        .bind(user_id)
        .fetch_one(pool)
        .await
}
