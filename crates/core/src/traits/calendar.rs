//! The calendar provider seam: one trait, so the sync is testable without Google.
//!
//! Google is the only place this phase writes to a remote system, and it does so behind this
//! trait so the sync can be driven by a fake (the roadmap's "providers are config, not code").
//! Two of the five methods exist for reasons a plausible implementation drops:
//!
//!  * [`CalendarApi::list_events`] takes a `sync_token`. `None` is a full listing and `Some`
//!    is incremental, and a token Google has expired is [`CalendarError::Gone`] rather than a
//!    transport error — the caller answers a `410` with a full resync instead of a retry
//!    (A236). Collapsing `Gone` into `Transport` turns a recoverable state into a poll that
//!    fails forever while every attempt looks like a network hiccup.
//!  * [`CalendarApi::upsert_event`] writes only into a calendar this app created, because the
//!    `calendar.app.created` scope cannot do anything else. That is what makes "Google wins
//!    for an event we did not create" structural rather than a rule someone has to remember
//!    (A234).

use chrono::{DateTime, Utc};

/// A remote calendar's identity.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CalendarMeta {
    pub id: String,
    pub summary: String,
    /// The remote's own copy of "this is a calendar we made", which is what
    /// [`CalendarApi::upsert_event`] is allowed to write to.
    pub primary: bool,
}

/// One remote event, in the shape the sync reads.
///
/// `start`/`end` are `Option` because Google's API has no required start: an event being
/// drafted, or one with only a `date` and no `dateTime`, arrives without the field. A struct
/// that insisted on one would have to invent a value, and an invented `starts_at` is an event
/// the student sees at the wrong time.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RemoteEvent {
    /// Google's opaque event id, verbatim: this is the `external_id` (A231).
    pub id: String,
    /// The row version. Stored so a later pass can tell whether the remote moved without
    /// re-reading every field.
    pub etag: String,
    /// `confirmed` | `tentative` | `cancelled`. A `cancelled` event is a tombstone, and it
    /// arrives through an *incremental* listing — which is exactly why a sync that only
    /// upserts silently keeps cancelled events forever.
    pub status: String,
    pub summary: String,
    pub start: Option<DateTime<Utc>>,
    pub end: Option<DateTime<Utc>>,
    /// Set on an occurrence of a recurring series. The row is the **occurrence**, so this is
    /// kept for diagnostics rather than used as a key (A231).
    pub recurring_event_id: Option<String>,
}

impl RemoteEvent {
    /// Whether this event is a cancellation rather than a live row.
    pub fn is_cancelled(&self) -> bool {
        self.status.eq_ignore_ascii_case("cancelled")
    }
}

/// One page of a listing, with the cursor for the next one.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct EventPage {
    pub events: Vec<RemoteEvent>,
    /// Google's `nextSyncToken`. `None` is a bug on Google's side for a successful listing,
    /// and the caller logs it rather than storing a `null` cursor that would make the next
    /// pass full without anyone noticing (A236).
    pub next_sync_token: Option<String>,
}

/// A refreshed access token.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AccessToken {
    pub token: String,
    pub expires_at: DateTime<Utc>,
}

/// Why a calendar call failed.
///
/// `Gone` is a variant of its own, not a `Transport` with a message: it is the one failure
/// whose remedy is a *different request* (a full resync) rather than a retry, and a caller
/// that has to parse a string to find that out will eventually get it wrong.
#[derive(Debug, Clone, thiserror::Error)]
pub enum CalendarError {
    /// The `sync_token` is no longer valid: the caller clears it and lists fully (A236).
    #[error("the sync cursor has expired; a full resync is required")]
    Gone,
    /// The grant is gone. Retrying cannot un-revoke it, so the connection is marked `revoked`
    /// and the pass succeeds.
    #[error("the calendar grant was refused: {0}")]
    Unauthorized(String),
    /// A network or timeout failure. The only variant a retry is worth.
    #[error("the calendar provider could not be reached: {0}")]
    Transport(String),
    /// The provider answered with something this code cannot use.
    #[error("the calendar provider answered with an error: {0}")]
    Provider(String),
}

impl CalendarError {
    /// Whether retrying this call could ever help.
    ///
    /// The distinction is the whole point of the enum: a `Transport` failure is worth five
    /// retries with backoff, while a `Gone` retried five times is five identical failures and
    /// half a minute of the worker's attention that a full resync would have used usefully.
    pub fn is_retryable(&self) -> bool {
        matches!(self, CalendarError::Transport(_))
    }
}

/// The remote calendar API, as the sync needs it.
///
/// `async_trait` rather than native `async fn` in trait: the trait is stored as
/// `Arc<dyn CalendarApi>` on `AppState`, and a `dyn`-compatible async trait needs the macro on
/// this toolchain.
#[async_trait::async_trait]
pub trait CalendarApi: Send + Sync {
    /// A listing. `sync_token: None` is a full listing; `Some` is incremental. A token Google
    /// has expired is [`CalendarError::Gone`], which the caller answers with a full resync
    /// rather than a retry (A236).
    async fn list_events(
        &self,
        calendar_id: &str,
        sync_token: Option<&str>,
    ) -> Result<EventPage, CalendarError>;

    /// The account's calendars, for finding "Study Plan" (A237).
    async fn list_calendars(&self) -> Result<Vec<CalendarMeta>, CalendarError>;

    /// Creates a calendar and returns it. Used once per connection, at connect (A237).
    async fn create_calendar(&self, summary: &str) -> Result<CalendarMeta, CalendarError>;

    /// `insert`/`patch` into a calendar this app created (`calendar.app.created`).
    async fn upsert_event(
        &self,
        calendar_id: &str,
        event: &RemoteEvent,
    ) -> Result<RemoteEvent, CalendarError>;

    /// Exchanges a refresh token for an access token.
    ///
    /// On the trait rather than inside the implementation because a fake needs to answer it
    /// without a network, and because the refresh token is the one secret this phase holds.
    async fn refresh_access_token(&self, refresh_token: &str) -> Result<AccessToken, CalendarError>;
}
