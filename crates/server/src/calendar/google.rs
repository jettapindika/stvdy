//! The Google Calendar client: [`CalendarApi`] over `reqwest` (Task 7).
//!
//! Three of this file's rules are the ones a plausible implementation drops, and each is
//! invisible in production:
//!
//!  * **`410` is [`CalendarError::Gone`]**, not a `Transport` with a message. It is the one
//!    failure whose remedy is a *different request* (a full listing) rather than a retry, and
//!    a caller that has to read a string to find that out will eventually get it wrong
//!    (A236). The check comes before the generic non-2xx arm, or the variant is unreachable.
//!  * **An incremental listing sends `showDeleted=true`.** A cancellation reaches us *only*
//!    through an incremental listing, and only when `showDeleted` is set: without it Google
//!    silently omits the tombstone and the local row stays live forever — the sync succeeds
//!    on every poll while the student sees an event that no longer exists.
//!  * **`nextSyncToken` is read from the last page.** It is present on the *final* page of a
//!    paged listing only, so a client that reads it from the first page stores `None` and
//!    silently turns every later pass into a full one (A236's exact failure: "an incremental
//!    sync that silently stopped being incremental").
//!
//! **What this file's behaviour is asserted against.** There is no Google credential on the
//! machine this was written on, so every test in `crates/server/tests/calendar.rs` drives
//! this client against a local HTTP stub and asserts the *request it sends* — the path, the
//! `syncToken`/`showDeleted` query parameters, the bearer header — and the mapping of the
//! stub's status onto [`CalendarError`]. Nothing here has been run against Google.

use parking_lot::Mutex;

use chrono::{DateTime, Duration, SecondsFormat, Utc};
use secrecy::ExposeSecret;
use serde_json::{Value, json};

use stvdy_core::traits::calendar::{
    AccessToken, CalendarApi, CalendarError, CalendarMeta, EventPage, RemoteEvent,
};

use crate::config::GoogleCalendarConfig;

/// The Calendar API's versioned root.
const GOOGLE_API_BASE: &str = "https://www.googleapis.com/calendar/v3";

/// The token endpoint — the same one the consent exchange posts to, because a refresh and a
/// code exchange are two `grant_type`s of one endpoint.
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// How long any single call to Google may take.
///
/// Bounded because the caller is a job worker: a request that hangs holds the pass, and the
/// poll's next tick has nothing to do with a socket that never answers. Thirty seconds is
/// generous for a listing of a few hundred events and short enough that a stuck pass is
/// noticed inside one poll interval.
const REQUEST_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(30);

/// The page size. Google's maximum, so a full listing of a busy calendar is one request
/// rather than five.
const PAGE_SIZE: &str = "250";

/// The Calendar API, as [`crate::calendar::sync::sync_google`] drives it.
pub struct GoogleCalendarApi {
    client: reqwest::Client,
    client_id: String,
    client_secret: String,
    /// `https://www.googleapis.com/calendar/v3` in production; overridable so the request
    /// shape is assertable against a local stub.
    api_base: String,
    /// `https://oauth2.googleapis.com/token` in production, for the same reason.
    token_url: String,
    /// The access token from the most recent [`CalendarApi::refresh_access_token`].
    ///
    /// **Why the client holds one at all.** The trait's `list_events` takes a calendar id and
    /// a cursor and no token, so the token has to reach the listing through the client rather
    /// than through the call. That is sound here and the reasons are both outside this file:
    /// the worker runs one job at a time per process (`jobs::worker`'s "one task per process"),
    /// and each process builds its own `AppState` and therefore its own client — so the most
    /// recent refresh is the running pass's. Two concurrent passes in one process would need
    /// the token as a parameter, which is a change to the seam rather than to this file, and
    /// the failure it would produce is a wrong-account listing rather than a leak: a token for
    /// another student's calendar cannot read this student's `study_calendar_id`.
    ///
    /// A `parking_lot` mutex rather than `std::sync`'s: the slot is never held across an `await`,
    /// so the only way it could be poisoned is a panic inside the two lines that touch it — and
    /// there is no remedy for that which a `Result` would express.
    access_token: Mutex<Option<AccessToken>>,
}

impl GoogleCalendarApi {
    /// The client for a configured deployment.
    pub fn new(google: &GoogleCalendarConfig) -> anyhow::Result<Self> {
        Self::with_endpoints(
            google.client_id.clone(),
            google.client_secret.expose_secret().to_string(),
            GOOGLE_API_BASE.to_string(),
            GOOGLE_TOKEN_ENDPOINT.to_string(),
        )
    }

    /// The same client against two other hosts.
    ///
    /// Public because the request shape is the contract and a local stub is the only way to
    /// assert it without a Google credential: a test that pointed the production endpoints at
    /// a stub would be reaching Google. The two are separate parameters because a real
    /// deployment's two hosts are separate, and a test needs them separate to answer a token
    /// request and an events request with two different bodies.
    pub fn with_endpoints(
        client_id: String,
        client_secret: String,
        api_base: String,
        token_url: String,
    ) -> anyhow::Result<Self> {
        let client = reqwest::Client::builder()
            .timeout(REQUEST_TIMEOUT)
            .build()
            .map_err(|error| {
                anyhow::anyhow!("building the Google Calendar http client: {error}")
            })?;

        Ok(Self {
            client,
            client_id,
            client_secret,
            api_base,
            token_url,
            access_token: Mutex::new(None),
        })
    }

    /// `{api_base}/{segments…}`, with every segment percent-encoded.
    ///
    /// Built through `url::Url` rather than by `format!`: a calendar id is usually an email
    /// address, and one containing a `#` or a `?` would otherwise be read as a fragment or a
    /// query rather than as part of the id.
    fn api_url(&self, segments: &[&str]) -> Result<url::Url, CalendarError> {
        let mut url = url::Url::parse(&format!("{}/", self.api_base.trim_end_matches('/')))
            .map_err(|error| {
                CalendarError::Provider(format!("the Google API base is not a URL: {error}"))
            })?;
        {
            let mut path = url.path_segments_mut().map_err(|_| {
                CalendarError::Provider("the Google API base cannot carry a path".to_string())
            })?;
            path.pop_if_empty();
            for segment in segments {
                path.push(segment);
            }
        }
        Ok(url)
    }

    /// One request, with Google's error taxonomy applied.
    ///
    /// The order is the taxonomy: `410` before the generic arm (or `Gone` is unreachable),
    /// `401`/`403` before it too, and everything else non-2xx is `Provider`. A body that is
    /// not JSON is a `Provider` error rather than a panic: an HTML error page from a proxy is
    /// a thing that happens, and it is not this code's fault or the caller's.
    async fn send(&self, request: reqwest::RequestBuilder) -> Result<Value, CalendarError> {
        let response = request
            .send()
            .await
            .map_err(|error| CalendarError::Transport(error.to_string()))?;
        let status = response.status();
        let body = response.text().await.map_err(|error| {
            CalendarError::Transport(format!("reading Google's answer: {error}"))
        })?;

        if status == reqwest::StatusCode::GONE {
            return Err(CalendarError::Gone);
        }
        if status == reqwest::StatusCode::UNAUTHORIZED || status == reqwest::StatusCode::FORBIDDEN {
            return Err(CalendarError::Unauthorized(reason(&body)));
        }
        if !status.is_success() {
            return Err(CalendarError::Provider(format!(
                "Google answered {}: {}",
                status.as_u16(),
                reason(&body)
            )));
        }

        serde_json::from_str(&body).map_err(|error| {
            CalendarError::Provider(format!("Google's answer could not be read: {error}"))
        })
    }

    /// The token the caller installed, refused when it is absent or already expired.
    ///
    /// The expiry is checked rather than trusted to the caller. A pass that listed with a
    /// token from an hour ago would get a `401`, which
    /// [`crate::calendar::sync::sync_google`] reads as a revoked grant and answers by marking
    /// the connection `revoked` — a live connection killed by a stale token. `Provider`
    /// rather than `Unauthorized` for both cases, because neither is Google refusing the
    /// grant: the caller's remedy is to refresh, and a `Provider` failure records the reason
    /// without revoking anything.
    fn access_token(&self) -> Result<String, CalendarError> {
        match self.access_token.lock().as_ref() {
            Some(token) if token.expires_at > Utc::now() => Ok(token.token.clone()),
            _ => Err(CalendarError::Provider(
                "no unexpired access token is held; refresh the grant first".to_string(),
            )),
        }
    }

    fn install(&self, token: AccessToken) {
        *self.access_token.lock() = Some(token);
    }
}

#[async_trait::async_trait]
impl CalendarApi for GoogleCalendarApi {
    async fn list_events(
        &self,
        calendar_id: &str,
        sync_token: Option<&str>,
    ) -> Result<EventPage, CalendarError> {
        let bearer = self.access_token()?;
        let mut events = Vec::new();
        let mut page_token: Option<String> = None;
        let mut next_sync_token: Option<String> = None;

        loop {
            let mut url = self.api_url(&["calendars", calendar_id, "events"])?;
            {
                let mut query = url.query_pairs_mut();
                // Always, not only when incremental: a full listing that omitted it would
                // re-adopt an event Google has already cancelled, and the row would come back
                // from the dead on the next full resync.
                query.append_pair("showDeleted", "true");
                // The **occurrence** is the row (A231), so a recurring series must arrive
                // expanded. `events` has no `rrule` column, and expanding on read would need a
                // second recurrence engine in the planner, the week grid and the feed.
                query.append_pair("singleEvents", "true");
                query.append_pair("maxResults", PAGE_SIZE);
                if let Some(sync_token) = sync_token {
                    query.append_pair("syncToken", sync_token);
                }
                if let Some(page_token) = &page_token {
                    query.append_pair("pageToken", page_token);
                }
            }

            let page = self.send(self.client.get(url).bearer_auth(&bearer)).await?;

            if let Some(items) = page.get("items").and_then(Value::as_array) {
                for item in items {
                    events.push(remote_event(item));
                }
            }
            // Only overwritten when present, which is what makes "the last page's cursor" true
            // rather than incidental: an intermediate page carries no `nextSyncToken` at all.
            if let Some(cursor) = page.get("nextSyncToken").and_then(Value::as_str) {
                next_sync_token = Some(cursor.to_string());
            }
            match page.get("nextPageToken").and_then(Value::as_str) {
                Some(next) => page_token = Some(next.to_string()),
                None => break,
            }
        }

        Ok(EventPage {
            events,
            next_sync_token,
        })
    }

    async fn list_calendars(&self) -> Result<Vec<CalendarMeta>, CalendarError> {
        let token = self.access_token()?;
        let mut calendars = Vec::new();
        let mut page_token: Option<String> = None;

        loop {
            let mut url = self.api_url(&["users", "me", "calendarList"])?;
            {
                let mut query = url.query_pairs_mut();
                query.append_pair("maxResults", PAGE_SIZE);
                // `showHidden=true`, and it is not a detail: the default hides calendars the
                // student has hidden, and "Study Plan" is exactly the calendar a student hides
                // once they are done with it (A237's own reason for a dedicated calendar). A
                // hidden one that we cannot see is a second "Study Plan" created on every
                // connect, and the study blocks split across two of them.
                query.append_pair("showHidden", "true");
                if let Some(page_token) = &page_token {
                    query.append_pair("pageToken", page_token);
                }
            }

            let page = self.send(self.client.get(url).bearer_auth(&token)).await?;
            if let Some(items) = page.get("items").and_then(Value::as_array) {
                for item in items {
                    calendars.push(CalendarMeta {
                        id: string_field(item, "id"),
                        summary: string_field(item, "summary"),
                        primary: item
                            .get("primary")
                            .and_then(Value::as_bool)
                            .unwrap_or(false),
                    });
                }
            }
            match page.get("nextPageToken").and_then(Value::as_str) {
                Some(next) => page_token = Some(next.to_string()),
                None => break,
            }
        }

        Ok(calendars)
    }

    async fn create_calendar(&self, summary: &str) -> Result<CalendarMeta, CalendarError> {
        let token = self.access_token()?;
        let url = self.api_url(&["calendars"])?;
        let body = self
            .send(
                self.client
                    .post(url)
                    .bearer_auth(&token)
                    .json(&json!({ "summary": summary })),
            )
            .await?;

        Ok(CalendarMeta {
            id: string_field(&body, "id"),
            summary: string_field(&body, "summary"),
            primary: body
                .get("primary")
                .and_then(Value::as_bool)
                .unwrap_or(false),
        })
    }

    async fn upsert_event(
        &self,
        calendar_id: &str,
        event: &RemoteEvent,
    ) -> Result<RemoteEvent, CalendarError> {
        let token = self.access_token()?;
        let mut body = json!({ "summary": event.summary });
        if let Some(start) = event.start {
            body["start"] = json!({ "dateTime": start.to_rfc3339_opts(SecondsFormat::Secs, true) });
        }
        if let Some(end) = event.end {
            body["end"] = json!({ "dateTime": end.to_rfc3339_opts(SecondsFormat::Secs, true) });
        }

        // The id decides which call this is, and it is the only thing that can: an event id
        // Google assigned is one Google knows (A231 stores it verbatim), while an empty one
        // means "this row has never been pushed" and Google assigns it. A `patch` on an id
        // Google does not know is a `404` — a `Provider` error rather than a silent insert,
        // because resurrecting an event the remote has deleted is a decision the caller has
        // to make, not one this method should make for it.
        if event.id.is_empty() {
            let url = self.api_url(&["calendars", calendar_id, "events"])?;
            let created = self
                .send(self.client.post(url).bearer_auth(&token).json(&body))
                .await?;
            return Ok(remote_event(&created));
        }

        let url = self.api_url(&["calendars", calendar_id, "events", &event.id])?;
        let updated = self
            .send(self.client.patch(url).bearer_auth(&token).json(&body))
            .await?;
        Ok(remote_event(&updated))
    }

    async fn refresh_access_token(
        &self,
        refresh_token: &str,
    ) -> Result<AccessToken, CalendarError> {
        let url = url::Url::parse(&self.token_url).map_err(|error| {
            CalendarError::Provider(format!("the Google token endpoint is not a URL: {error}"))
        })?;

        let body = self
            .send(self.client.post(url).form(&[
                ("client_id", self.client_id.as_str()),
                ("client_secret", self.client_secret.as_str()),
                ("refresh_token", refresh_token),
                ("grant_type", "refresh_token"),
            ]))
            .await?;

        let token = body
            .get("access_token")
            .and_then(Value::as_str)
            .ok_or_else(|| {
                CalendarError::Provider("Google's token answer carried no access_token".to_string())
            })?;
        // Google always sends `expires_in`; the hour is the documented lifetime and the value
        // to fall back to rather than treating a missing field as "never expires".
        let expires_in = body
            .get("expires_in")
            .and_then(Value::as_i64)
            .unwrap_or(3600);

        let token = AccessToken {
            token: token.to_string(),
            expires_at: Utc::now() + Duration::seconds(expires_in),
        };
        self.install(token.clone());
        Ok(token)
    }
}

/// One event, as the sync reads it.
///
/// A missing `id`, `etag`, `status` or `summary` becomes an empty string rather than a
/// refusal: an event being drafted arrives without a summary, and refusing the whole page over
/// it would drop every other event with it. `status` empty is not a cancellation — only
/// Google's own `cancelled` is — so an event with no status is a live row.
fn remote_event(item: &Value) -> RemoteEvent {
    RemoteEvent {
        id: string_field(item, "id"),
        etag: string_field(item, "etag"),
        status: string_field(item, "status"),
        summary: string_field(item, "summary"),
        start: instant(item.get("start")),
        end: instant(item.get("end")),
        recurring_event_id: item
            .get("recurringEventId")
            .and_then(Value::as_str)
            .map(str::to_string),
    }
}

fn string_field(item: &Value, key: &str) -> String {
    item.get(key)
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string()
}

/// The instant a `start`/`end` names, or `None`.
///
/// **`None` for a date-only value**, deliberately, and it is the reason the trait's field is
/// an `Option`: an all-day event arrives as `{"date": "2026-06-10"}` and has no instant in it.
/// Reading that as midnight UTC would place a Jakarta all-day event at 07:00 local — an event
/// the student sees at the wrong time, which is worse than one this pass cannot place. The
/// sync skips an event with no instant and says so in the log.
fn instant(value: Option<&Value>) -> Option<DateTime<Utc>> {
    let raw = value?.get("dateTime")?.as_str()?;
    DateTime::parse_from_rfc3339(raw)
        .ok()
        .map(|at| at.with_timezone(&Utc))
}

/// A readable reason out of Google's error body.
///
/// Two shapes, because Google has two: an API error is `{"error": {"code": …, "message": …}}`
/// and the token endpoint's is `{"error": "invalid_grant", "error_description": …}`.
/// `invalid_grant` versus `redirect_uri_mismatch` is the difference between a five-minute fix
/// and an afternoon, so the description is preferred over the code and the code over the raw
/// body.
fn reason(body: &str) -> String {
    let Ok(value) = serde_json::from_str::<Value>(body) else {
        return truncate(body);
    };
    let error = &value["error"];
    if let Some(message) = error.get("message").and_then(Value::as_str) {
        return message.to_string();
    }
    if let Some(description) = value.get("error_description").and_then(Value::as_str) {
        return description.to_string();
    }
    if let Some(code) = error.as_str() {
        return code.to_string();
    }
    truncate(body)
}

/// A body, bounded so an HTML error page does not become a log line.
fn truncate(body: &str) -> String {
    let trimmed = body.trim();
    if trimmed.chars().count() <= 200 {
        return trimmed.to_string();
    }
    trimmed.chars().take(200).collect()
}
