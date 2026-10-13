//! The ICS feed out: `GET /cal/{token}.ics`, and the token that addresses it.
//!
//! The feed itself is a pure function of rows (`stvdy_core::calendar::feed`), so this
//! module is a token lookup, one `select`, and three headers. The three things it does own:
//!
//!  * **The route shape.** The path is registered as `/cal/{token}` and the handler requires
//!    the segment to end in `.ics`, because the router cannot express a static suffix after a
//!    parameter: `matchit` 0.8 rejects `{token}.ics` at insert time ("prefixes after route
//!    parameters are not supported"), which is a panic in `router()`, not a 404. So the
//!    documented URL is what the client builds and the `.ics` is stripped here — and a request
//!    without it is a 404 rather than a second URL for the same resource.
//!  * **The token is the credential** (A242). It is stored in plaintext in
//!    `users.ics_feed_token` because the settings screen has to show the link again, so this
//!    is a capability URL: what bounds it is that the feed is read-only, carries only the
//!    student's own app-owned events, and is revoked and replaced in one click. It is never
//!    logged (A243) — a secret in a `tracing` field is a secret in every log aggregator.
//!  * **What the feed carries** (A241): `source = 'app'`, not tombstoned, and
//!    `kind in (class, exam, study_block, deadline)`, plus one synthetic deadline per task
//!    with a `due_at`. `kind = 'external'` and every `source != 'app'` row are excluded: they
//!    came *from* a calendar, and echoing them back would show every Google event twice in
//!    the student's own client, under a different `UID`, with no way to tell the two apart.

use axum::Json;
use axum::extract::{Path, State};
use axum::http::{HeaderName, header};
use axum::response::{IntoResponse, Response};
use base64::Engine;
use chrono::{DateTime, Utc};
use chrono_tz::Tz;
use rand::Rng;
use serde_json::{Value, json};
use sqlx::PgPool;
use uuid::Uuid;

use stvdy_core::calendar::feed::{FeedEvent, FeedKind, render_feed};

use crate::auth::extract::VerifiedUser;
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The suffix the feed URL carries. Stripped before the lookup, and required rather than
/// optional so one token has exactly one URL.
const FEED_SUFFIX: &str = ".ics";

/// The kinds the feed carries (A241). Written out because the SQL filter is a literal:
/// `kind` is a `text` column with no `check` constraint, so the set exists only where it is
/// read.
const FEED_KINDS: &[&str] = &["class", "exam", "study_block", "deadline"];

/// `GET /cal/{token}.ics`. No bearer token: the path token is the credential (A242).
///
/// Logs the resolved `user_id` and the byte length, never the token (A243). A token that
/// matches nothing is a 404 from an index hit or a miss — never a 500, because the lookup is
/// a unique-index probe on a `text` column and a malformed value is the same miss as an
/// unknown one.
pub async fn feed(
    State(state): State<AppState>,
    Path(segment): Path<String>,
) -> AppResult<Response> {
    let Some(token) = segment.strip_suffix(FEED_SUFFIX) else {
        return Err(AppError::NotFound);
    };
    if token.is_empty() {
        return Err(AppError::NotFound);
    }

    let row: Option<(Uuid, String)> = sqlx::query_as(
        "select id, timezone from users where ics_feed_token = $1 and deleted_at is null",
    )
    .bind(token)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("resolving a feed token: {error}")))?;

    let Some((user_id, timezone)) = row else {
        return Err(AppError::NotFound);
    };

    let events = feed_rows(&state.pool, user_id, resolve_zone(user_id, &timezone)).await?;
    let body = render_feed(&events);

    // The token is deliberately absent from this line: it is the whole credential.
    tracing::info!(%user_id, bytes = body.len(), events = events.len(), "served the ICS feed");

    Ok((
        [
            (
                header::CONTENT_TYPE,
                "text/calendar; charset=utf-8".to_string(),
            ),
            // A feed is a cache the *client* owns and re-reads on its own schedule, so there
            // is nothing here for a proxy to usefully hold — and a stale copy served to a
            // revoked token is exactly the leak the rotation exists to close.
            (header::CACHE_CONTROL, "no-store".to_string()),
            // The URL is a secret, and a subscription URL is the kind of thing a crawler
            // finds through a shared link, a referrer or a chat preview.
            (
                HeaderName::from_static("x-robots-tag"),
                "noindex".to_string(),
            ),
        ],
        body,
    )
        .into_response())
}

/// `POST /calendar/ics-token/regenerate`. Authenticated. Writes 32 fresh random bytes,
/// URL-safe base64, and returns `{ "ics_feed_token": "..." }`.
///
/// Rotation is revoke-and-replace in one statement: the old value is overwritten, so a
/// client holding the previous URL 404s on its next poll. That is the whole remedy A242
/// promises, and it is why the token is a column rather than a signed value — a signed feed
/// URL could not be revoked without rotating a server-wide key.
pub async fn regenerate_token(
    State(state): State<AppState>,
    user: VerifiedUser,
) -> AppResult<Json<Value>> {
    let token = feed_token();
    sqlx::query("update users set ics_feed_token = $2 where id = $1")
        .bind(user.user_id)
        .bind(&token)
        .execute(&state.pool)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("rotating the feed token: {error}")))?;

    // No token in the log line here either: this one is live.
    tracing::info!(user_id = %user.user_id, "rotated the ICS feed token");
    Ok(Json(json!({ "ics_feed_token": token })))
}

/// 32 random bytes, URL-safe base64 with no padding — the same shape and the same source of
/// randomness as the login flow's ticket, because both are bearer secrets of the same size.
///
/// 32 bytes rather than 16: this one is guessable-by-brute-force in a way a login ticket is
/// not, because the URL is public, unauthenticated and permanent. 2^256 over a 30-requests-a
/// -minute bucket is not a search space.
fn feed_token() -> String {
    let mut bytes = [0u8; 32];
    rand::rng().fill_bytes(&mut bytes);
    base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes)
}

/// `GET /calendar/ics-token`. Authenticated. The caller's current feed token, or `null`
/// when none has been generated.
///
/// Not one of the spec's seven routes, and required anyway: A242 states that the token is
/// "readable again by the settings screen" — the screen the spec names (item 13) shows
/// "copy 'your calendar feed' link with Regenerate" — and a screen that could only obtain
/// the link by *rotating* it would break every subscribed device each time the student
/// opened settings. A249 is the same kind of addition for the same kind of reason.
///
/// It returns the caller's own token to the caller's own session and nothing else, which is
/// why it is a `ReadOnly` row in the route sweep.
pub async fn token(State(state): State<AppState>, user: VerifiedUser) -> AppResult<Json<Value>> {
    let token: Option<String> =
        sqlx::query_scalar("select ics_feed_token from users where id = $1 and deleted_at is null")
            .bind(user.user_id)
            .fetch_optional(&state.pool)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("reading the feed token: {error}"))
            })?
            .flatten();

    Ok(Json(json!({ "ics_feed_token": token })))
}

/// The rows the feed carries (A241), in one statement with an explicit column list (A27).
///
/// Two sources in a `union all`: the student's own events, and one synthetic deadline per
/// task that has a `due_at`. The synthetic row is what makes "tasks with due dates" true — a
/// task is not an `events` row (Phase 9 writes no event for one), and materialising a
/// deadline event per task would be a second write path for a value the task already holds.
///
/// `coalesce(created_at, updated_at)` because both tables declare `created_at` as
/// `default now()` with no `not null`, so a row written with an explicit null is legal and
/// `FeedEvent` has no room for one. `SEQUENCE` is derived from that pair (A230), so a null
/// would be a null in the one value a client caches on.
///
/// Ordered by `starts_at` then `id` so two renders of an unchanged set are byte-identical:
/// `render_feed` writes the components in the order it is handed them, and a feed that
/// reorders on every poll is a feed every client re-imports.
async fn feed_rows(pool: &PgPool, user_id: Uuid, timezone: Tz) -> AppResult<Vec<FeedEvent>> {
    let rows: Vec<FeedRow> = sqlx::query_as(
        "select e.id, e.kind, e.title, e.starts_at, e.ends_at,
                coalesce(e.created_at, e.updated_at) as created_at, e.updated_at
           from events e
          where e.user_id = $1 and e.deleted_at is null
            and e.source = 'app'
            and e.kind = any($2)
         union all
         select t.id, 'deadline', t.title, t.due_at, t.due_at,
                coalesce(t.created_at, t.updated_at), t.updated_at
           from tasks t
          where t.user_id = $1 and t.deleted_at is null and t.due_at is not null
          order by 4 asc, 1 asc",
    )
    .bind(user_id)
    .bind(FEED_KINDS)
    .fetch_all(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("reading the feed rows: {error}")))?;

    let mut events = Vec::with_capacity(rows.len());
    for row in rows {
        // The query's filter is the set, so an unknown kind cannot arrive; the arm exists so
        // a row this build does not understand is dropped rather than rendered under a kind
        // the client would draw wrongly.
        let Some(kind) = feed_kind(&row.kind) else {
            tracing::warn!(kind = %row.kind, event_id = %row.id, "skipping a feed row with an unknown kind");
            continue;
        };
        events.push(FeedEvent {
            id: row.id,
            kind,
            title: row.title,
            starts_at: row.starts_at,
            ends_at: row.ends_at,
            created_at: row.created_at,
            updated_at: row.updated_at,
            timezone,
        });
    }
    Ok(events)
}

/// The shape the `union all` returns. `kind` is the raw column value, mapped by [`feed_kind`].
#[derive(sqlx::FromRow)]
struct FeedRow {
    id: Uuid,
    kind: String,
    title: String,
    starts_at: DateTime<Utc>,
    ends_at: DateTime<Utc>,
    created_at: DateTime<Utc>,
    updated_at: DateTime<Utc>,
}

/// The column's value as the feed's kind.
fn feed_kind(raw: &str) -> Option<FeedKind> {
    match raw {
        "class" => Some(FeedKind::Class),
        "exam" => Some(FeedKind::Exam),
        "study_block" => Some(FeedKind::StudyBlock),
        "deadline" => Some(FeedKind::Deadline),
        _ => None,
    }
}

/// The user's IANA zone, through `chrono-tz` and never the machine's (A6).
///
/// Only a deadline reads it (A248): the date-only `DTSTART` has to be the student's own
/// Friday. An unparseable value falls back to the spec's default zone rather than to UTC,
/// because a Jakarta deadline rendered in UTC is a day early — and the fallback is logged,
/// since a timezone the tz database does not know is a data problem nobody would otherwise
/// see.
fn resolve_zone(user_id: Uuid, timezone: &str) -> Tz {
    match timezone.parse::<Tz>() {
        Ok(zone) => zone,
        Err(_) => {
            tracing::warn!(
                %user_id,
                timezone,
                "an unknown timezone on a feed row; falling back to the default zone"
            );
            chrono_tz::Asia::Jakarta
        }
    }
}
