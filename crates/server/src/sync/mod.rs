//! The sync routes: pull, push, stream, plus the compaction job's shared helpers.
//!
//! The per-mutation decision is NOT here. `sync::push::decide` landed in Phase 7 (A150)
//! and this module adds only the transport around it: the advisory lock, the
//! idempotency log, the apply, and the results.

pub mod log;
pub mod pull;
pub mod push;

use axum::Router;
use axum::routing::get;
use sqlx::PgConnection;
use uuid::Uuid;

use crate::state::AppState;

pub fn routes() -> Router<AppState> {
    // One `.route(...)` per task that lands it: `post("/sync/push", …)` in Task 4 and
    // `get("/sync/stream", …)` in Task 6 join this call, so the module never declares a
    // handler that does not exist yet.
    Router::new().route("/sync/pull", get(pull::pull))
}

/// Records that this device just talked to the server.
///
/// The Devices screen's "Last synced" reads `devices.last_sync_at`, and nothing wrote
/// it before this phase (A180). A locally invented time would be a number the app did
/// not compute, which is what roadmap gate 7(a) forbids. Takes the caller's own
/// connection so the write joins the transaction that did the work: a pull that fails
/// after this must not leave a "synced" timestamp behind.
pub async fn touch_last_sync(conn: &mut PgConnection, device_id: Uuid) -> sqlx::Result<()> {
    sqlx::query("update devices set last_sync_at = now(), last_seen_at = now() where id = $1")
        .bind(device_id)
        .execute(conn)
        .await
        .map(|_| ())
}
