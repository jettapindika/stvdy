//! The SSE writer for chat (roadmap § 4).
//!
//! Hand-rolled rather than `axum::response::sse`'s typed helper because the
//! contract has seven named events and one of them (`error`) has to be sendable
//! at any point, including before the first token. Keeping the frame shape in
//! one place is also what stops a handler from inventing an eighth event name.

use axum::response::sse::Event;
use stvdy_core::citations::Citation;

/// How often a comment heartbeat goes out. A proxy or a load balancer closes an
/// idle connection, and a model that is thinking produces no tokens for a while,
/// so the stream would die mid-answer.
pub const HEARTBEAT: std::time::Duration = std::time::Duration::from_secs(15);

/// `event: token` with one text fragment.
pub fn token(text: &str) -> Event {
    Event::default()
        .event("token")
        .json_data(serde_json::json!({ "t": text }))
        .unwrap_or_else(|_| fallback())
}

/// `event: reasoning` with one reasoning fragment from chain-of-thought.
/// Sent only when the provider exposes it; a client that never sees this event
/// simply has no thinking disclosure to show, which is the correct empty state.
pub fn reasoning(text: &str) -> Event {
    Event::default()
        .event("reasoning")
        .json_data(serde_json::json!({ "t": text }))
        .unwrap_or_else(|_| fallback())
}

/// `event: citations` with the resolved, structured array. Sent even when empty:
/// a client that only ever sees this event on success cannot tell "no citations"
/// from "the stream died before the citations event".
pub fn citations(citations: &[Citation]) -> Event {
    Event::default()
        .event("citations")
        .json_data(serde_json::json!({ "citations": citations }))
        .unwrap_or_else(|_| fallback())
}

/// `event: usage` with what the message cost and what is left.
pub fn usage(credits_spent: i64, credits_remaining: i64) -> Event {
    Event::default()
        .event("usage")
        .json_data(serde_json::json!({
            "credits_spent": credits_spent,
            "credits_remaining": credits_remaining,
        }))
        .unwrap_or_else(|_| fallback())
}

/// `event: done` with the persisted message id.
pub fn done(message_id: uuid::Uuid, used_web: bool) -> Event {
    Event::default()
        .event("done")
        .json_data(serde_json::json!({
            "message_id": message_id,
            "used_web": used_web,
        }))
        .unwrap_or_else(|_| fallback())
}

/// `event: error` with the same five-key body every other route uses, so the
/// client's error mapper needs no second code path.
pub fn error(error: &crate::error::AppError) -> Event {
    let mut body = serde_json::json!({
        "code": error.code(),
        "message": error.to_string(),
        "min_plan": serde_json::Value::Null,
        "limit": serde_json::Value::Null,
        "used": serde_json::Value::Null,
    });

    if let crate::error::AppError::Entitlement {
        min_plan,
        limit,
        used,
        ..
    } = error
    {
        body["min_plan"] = serde_json::json!(min_plan);
        body["limit"] = serde_json::json!(limit);
        body["used"] = serde_json::json!(used);
    }

    Event::default()
        .event("error")
        .json_data(body)
        .unwrap_or_else(|_| fallback())
}

/// `event: action` for a WRITE tool's proposal (A277).
///
/// The body is the roadmap § 4 shape and nothing else: `{"pending_action_id", "tool", "args"}`.
/// Adding a `cost` field would be a contract change made by a phase plan — the card reads the
/// action's price from `app/src/features/billing/plans.ts`, the same source every other cost
/// on screen comes from.
///
/// The event is emitted at the moment the proposal row exists, so the client can render the
/// card immediately rather than waiting for `done`: a turn that proposed something may
/// produce no further tokens at all (A281), and a card that arrived only with `done` would
/// leave the student watching an empty turn.
pub fn action(pending_action_id: uuid::Uuid, tool: &str, args: &serde_json::Value) -> Event {
    let body = serde_json::json!({
        "pending_action_id": pending_action_id,
        "tool": tool,
        "args": args,
    });
    Event::default().event("action").data(body.to_string())
}

/// A comment line. `Event::default().comment` is what makes the proxy see bytes
/// without the client seeing an event.
pub fn heartbeat() -> Event {
    Event::default().comment("keep-alive")
}

/// A last-resort frame if serialization somehow fails.
///
/// `json_data` on a `serde_json::Value` cannot fail, so this is unreachable in
/// practice; it exists because the alternative is a `Result` unwrap in a stream
/// that has no way to report an error, and a silent drop would be worse than a
/// visible one.
fn fallback() -> Event {
    Event::default()
        .event("error")
        .data(r#"{"code":"internal","message":"failed to encode an event","min_plan":null,"limit":null,"used":null}"#)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The event names are the client's contract, so they are asserted rather
    /// than trusted to stay spelled the same.
    #[test]
    fn the_event_names_match_the_documented_contract() {
        let names = ["token", "reasoning", "citations", "usage", "done", "error"];
        for name in names {
            let event = match name {
                "token" => token("x"),
                "reasoning" => reasoning("thinking"),
                "citations" => citations(&[]),
                "usage" => usage(1, 9),
                "done" => done(uuid::Uuid::now_v7(), false),
                _ => error(&crate::error::AppError::Unauthorized),
            };
            let rendered = format!("{event:?}");
            assert!(
                rendered.contains(name),
                "the {name} event must carry its own name"
            );
        }
    }

    #[test]
    fn a_heartbeat_is_a_comment_not_an_event() {
        let rendered = format!("{:?}", heartbeat());
        assert!(rendered.contains("keep-alive"));
        assert!(
            !rendered.contains("event"),
            "a heartbeat must be a comment line, or a client's parser would try to read it as an event"
        );
    }
}
