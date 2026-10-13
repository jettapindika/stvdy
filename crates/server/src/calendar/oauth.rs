//! The Google Calendar consent flow: `GET /calendar/google/connect` and `/callback` (Task 6).
//!
//! Built on the **login flow's** state cipher (`crate::auth::oauth_state`) rather than a
//! second OAuth path (A235): both flows carry a PKCE verifier through a browser round trip,
//! and a second AES-GCM `state` codec is a second place to get the nonce handling wrong. The
//! two flows share the codec and not a trust domain — `Purpose::Calendar` inside the
//! ciphertext is what refuses a login `state` replayed here, and vice versa.
//!
//! ## The client is a different one from the login client
//!
//! `config.google_calendar` is its own three variables. One client for both flows would mean
//! the sign-in screen requesting calendar write access, which is the thing the spec's
//! "a page this app draws around someone else's login form" rule exists to prevent — and the
//! scopes are the other half of it (see [`GOOGLE_CALENDAR_SCOPES`]).
//!
//! ## What this module cannot do in this build
//!
//! **The live half.** The OAuth client does not exist yet, so no exchange against Google has
//! ever run here. Everything that does not need Google is implemented and tested: the consent
//! URL's parameters, the state round trip, the refusals, and the shape of the deep link. The
//! step that needs the provider — finding or creating the dedicated "Study Plan" calendar
//! (A237) — is [`ensure_study_calendar`], which reaches Google through the `CalendarApi` seam
//! in `crates/server/src/calendar/google.rs`, and which is therefore asserted against a fake
//! client rather than against Google.

use aes_gcm::Nonce;
use aes_gcm::aead::Aead;
use axum::Json;
use axum::extract::{Query, State};
use axum::response::IntoResponse;
use secrecy::ExposeSecret;
use serde::Deserialize;
use serde_json::{Value, json};
use uuid::Uuid;

use stvdy_core::entitlements::Feature;

use crate::auth::extract::VerifiedUser;
use crate::auth::oauth_state::{
    Purpose, StatePayload, decrypt_state, encrypt_state, pkce_pair, random_token, state_expiry,
};
use crate::error::{AppError, AppResult};
use crate::state::AppState;

/// The consent's two scopes, and the reason they are these two (A234).
///
/// `calendar.app.created` can create and manage **only** calendars this app made, which is
/// what makes "our study blocks go to a dedicated 'Study Plan' calendar" (A237) structural
/// rather than a rule someone has to remember; `calendar.events.readonly` can read everything
/// else and change nothing. So "Google wins for an event we did not create" is enforced by
/// the token, and a bug in the merge logic cannot rewrite a user's own event.
///
/// A constant rather than config: a deployment that widened these would silently move that
/// guarantee, which is not a deployment choice.
pub const GOOGLE_CALENDAR_SCOPES: &str = "https://www.googleapis.com/auth/calendar.app.created \
     https://www.googleapis.com/auth/calendar.events.readonly";

const GOOGLE_AUTH_ENDPOINT: &str = "https://accounts.google.com/o/oauth2/v2/auth";
const GOOGLE_TOKEN_ENDPOINT: &str = "https://oauth2.googleapis.com/token";

/// The deep link the callback redirects to.
///
/// `stvdy://oauth/calendar` is the route `src-tauri/src/deeplink.rs` classifies as
/// `DeepLinkIntent::Calendar`, and the SPA's `DeepLinkHandler` refreshes the connections
/// query on `status=connected` and reports the reason otherwise. **No token of any kind
/// travels here** (A235/A48): the refresh token was written server-side a few lines earlier,
/// and a deep link lands in browser history, referrer logs and the OS URL handler.
const DEEP_LINK: &str = "stvdy://oauth/calendar";

/// `GET /calendar/google/connect`. Returns `{ "url" }` for the SPA to open with
/// `platform.openExternal` — the **system browser**, never an embedded webview.
///
/// 404 when the calendar client is not configured, the same rule `auth::google::start`
/// follows: a half-configured client must not produce a redirect that fails at Google with a
/// message the student cannot act on.
///
/// Two entitlement checks, in this order (A245): the plan's boolean first, then the counted
/// account cap. A free account has `google_calendar = false` **and** `google_accounts_max = 0`,
/// and "Google Calendar is not included in your current plan" is the useful of the two —
/// "0 of 0 Google accounts" reads like a bug rather than a refusal.
pub async fn connect(State(state): State<AppState>, user: VerifiedUser) -> AppResult<Json<Value>> {
    let Some(google) = &state.config.google_calendar else {
        return Err(AppError::NotFound);
    };

    crate::entitlements::require(&state, &user, Feature::GoogleCalendar).await?;
    crate::entitlements::require(&state, &user, Feature::GoogleAccount).await?;

    let (verifier, challenge) = pkce_pair();
    let payload = StatePayload {
        purpose: Purpose::Calendar,
        verifier,
        nonce: random_token(),
        exp: state_expiry(),
        // The whole difference from a login state: this flow is started by a signed-in
        // request and must return the grant to *that* account. Carried in the encrypted
        // state rather than in a server session, which keeps the flow stateless and
        // therefore correct behind more than one instance.
        user_id: Some(user.user_id),
        // Unused by this flow: nothing here creates a device.
        device_name: String::new(),
        device_platform: String::new(),
        app_version: None,
    };
    let state_param = encrypt_state(&state, &payload)?;

    let url = url::Url::parse_with_params(
        GOOGLE_AUTH_ENDPOINT,
        &[
            ("client_id", google.client_id.as_str()),
            ("redirect_uri", google.redirect_uri.as_str()),
            ("response_type", "code"),
            ("scope", GOOGLE_CALENDAR_SCOPES),
            ("state", state_param.as_str()),
            ("code_challenge", challenge.as_str()),
            ("code_challenge_method", "S256"),
            // A refresh token is the whole point of this flow, and Google returns one only
            // on a first consent unless `prompt=consent` asks for it again — so a re-connect
            // after a revoke would otherwise come back with no token at all and the
            // connection would be dead on arrival.
            ("access_type", "offline"),
            ("prompt", "consent"),
        ],
    )
    .map_err(|error| AppError::Internal(anyhow::anyhow!("building the Google URL: {error}")))?;

    Ok(Json(json!({ "url": url.to_string() })))
}

#[derive(Debug, Deserialize)]
pub struct CallbackQuery {
    pub code: Option<String>,
    pub state: Option<String>,
    /// Google's own refusal parameter (`access_denied` when the student declines).
    pub error: Option<String>,
}

/// `GET /calendar/google/callback`. Public: a browser redirect carries no bearer token, so
/// this route is `Exempt` in the route sweep.
///
/// **It answers with a redirect in every case, including its refusals.** The browser is
/// sitting on Google's redirect URI, and a JSON error page there is a dead end: the student
/// has no way back to the app and no idea what happened. So a refusal is a redirect carrying
/// `status=error` and a reason, which is exactly what the SPA's `DeepLinkHandler` renders.
///
/// The steps, and the order is the security argument:
///
/// 1. the state decrypts **as a calendar state**, which refuses a forged one, one minted by
///    the login flow, and an expired one;
/// 2. the code is exchanged with the state's own PKCE verifier;
/// 3. the refresh token is encrypted and the connection row written — after a **second**
///    entitlement check through `require_for`, because the cap has to hold where the row is
///    written and not only where the flow was started: between the two, a student on another
///    device could have taken the last account slot;
/// 4. the deep link carries a status and a reason, never a token.
///
/// **The "Study Plan" calendar (A237) is found or created here**, by
/// [`ensure_study_calendar`], once the grant is stored — and a failure to resolve it is
/// **recorded on the connection rather than raised**: the token is already written, the
/// student's own Google events still import, and a 500 would tell a student whose grant was
/// stored that nothing happened. The connection's `error` carries the reason (A247).
///
/// **A replayed `state` is refused by Google, not by us.** The state is stateless (that is
/// what makes it work on more than one instance), so it carries no single-use marker; the
/// authorization code inside it is single-use at Google's end, and a second exchange of the
/// same code is an `invalid_grant`. What this code *does* refuse is the replay that no third
/// party can: a state minted by the other flow, and a tampered or expired one.
pub async fn callback(
    State(state): State<AppState>,
    Query(query): Query<CallbackQuery>,
) -> AppResult<axum::response::Response> {
    let Some(google) = &state.config.google_calendar else {
        return Err(AppError::NotFound);
    };

    if let Some(error) = query.error.as_deref().filter(|value| !value.is_empty()) {
        // The student declined, or Google refused. Not an internal error, and not a 4xx
        // either: the browser needs to be sent back to the app.
        return deep_link("denied", Some(error));
    }

    let Some(encoded) = query.state.as_deref().filter(|value| !value.is_empty()) else {
        return deep_link("error", Some("missing OAuth state"));
    };
    let payload = match decrypt_state(&state, Purpose::Calendar, encoded) {
        Ok(payload) => payload,
        Err(error) => {
            tracing::info!(%error, "a calendar consent callback presented a state we refuse");
            return deep_link("error", Some("the consent request is not valid"));
        }
    };
    let Some(user_id) = payload.user_id else {
        // Unreachable through `connect`, which always sets it. A state with no account
        // cannot be acted on, and guessing one would connect a calendar to the wrong
        // student.
        return deep_link("error", Some("the consent request names no account"));
    };

    let Some(code) = query.code.as_deref().filter(|value| !value.is_empty()) else {
        return deep_link("error", Some("missing authorization code"));
    };

    let token = match exchange_code(google, code, &payload.verifier).await {
        Ok(token) => token,
        Err(error) => {
            // The reason is logged in full and reported coarsely: the body may name a
            // `redirect_uri_mismatch`, which is an operator's problem, and the deep link is
            // rendered to the student.
            tracing::warn!(%error, %user_id, "the Google Calendar code exchange failed");
            return deep_link("error", Some("Google refused the connection"));
        }
    };

    let Some(refresh_token) = token.get("refresh_token").and_then(Value::as_str) else {
        // `access_type=offline` plus `prompt=consent` should always return one; a response
        // without it cannot keep a connection alive past the first hour.
        tracing::warn!(%user_id, "Google returned no refresh token for the calendar grant");
        return deep_link("error", Some("Google did not grant offline access"));
    };

    // The cap is enforced **where the row is written** (A245). `require_for` is the
    // job-shaped arm of the same engine: there is no `AuthUser` here, only the account the
    // state names, and the decision comes from the same snapshot the request path uses.
    if let Err(error) =
        crate::entitlements::require_for(&state, user_id, Feature::GoogleCalendar).await
    {
        tracing::info!(%user_id, code = error.code(), "a calendar connect was refused after consent");
        return deep_link(
            "error",
            Some("Google Calendar is not available on your plan"),
        );
    }
    if let Err(error) =
        crate::entitlements::require_for(&state, user_id, Feature::GoogleAccount).await
    {
        tracing::info!(%user_id, code = error.code(), "the Google account cap refused a connect");
        return deep_link(
            "error",
            Some("your plan's Google account limit is already in use"),
        );
    }

    let encrypted = crate::auth::oauth_state::encrypt_secret(&state, refresh_token)?;
    let connection_id = store_connection(&state, user_id, &encrypted).await?;

    // A237: the "Study Plan" calendar is found or created **once**, while the student is
    // watching, so a connect that cannot have one says so now rather than silently dropping the
    // first study block later. It cannot fail the connect: the grant is stored above, and the
    // sync lists `primary` when there is no study calendar, so the cost of a failure here is
    // the mirror destination rather than the connection. A database failure while *recording*
    // that reason is logged rather than raised for the same reason — the student's grant is
    // live, and an error page would say otherwise.
    if let Err(error) = ensure_study_calendar(&state, connection_id).await {
        tracing::warn!(%user_id, %error, "the Study Plan calendar could not be recorded");
    }

    tracing::info!(%user_id, "Google Calendar connected");
    deep_link("connected", None)
}

/// The name of the calendar the mirrored study blocks live in (A237).
///
/// Exactly this string, and matched exactly: "Study Plan (2)" is a calendar somebody else made,
/// and adopting it would write our study blocks into one the student believes is theirs alone.
pub const STUDY_CALENDAR_SUMMARY: &str = "Study Plan";

/// Finds the connection's "Study Plan" calendar, creating it when there is none, and stores its
/// id (A237).
///
/// **A failure here is not a failed connect.** The grant is already stored, so the connection
/// works: the student's own Google events import either way, because the sync lists `primary`
/// when there is no study calendar. What a failure costs is the mirror destination, so the
/// reason is recorded in `calendar_connections.error` — which the screen shows beside the
/// connection (A247, gate 7) — and the connection stays `connected`. That is the whole reason
/// this is a separate function from [`store_connection`]: the two have different failure
/// meanings, and a caller that ran them as one would have to choose between a lost grant and a
/// 500.
///
/// Public because it is the unit the tests drive: reaching it through the callback would need a
/// real authorization-code exchange with Google, and what is being asserted is this step's own
/// decision — find, create, or record.
pub async fn ensure_study_calendar(state: &AppState, connection_id: Uuid) -> AppResult<()> {
    let Some(api) = state.calendar.as_ref() else {
        return record_connect_error(
            state,
            connection_id,
            "no Google Calendar client is configured on this deployment",
        )
        .await;
    };

    let found = match api.list_calendars().await {
        Ok(calendars) => calendars
            .into_iter()
            .find(|calendar| calendar.summary == STUDY_CALENDAR_SUMMARY),
        Err(error) => {
            return record_connect_error(
                state,
                connection_id,
                &format!("the calendar list could not be read: {error}"),
            )
            .await;
        }
    };

    let calendar = match found {
        Some(calendar) => calendar,
        None => match api.create_calendar(STUDY_CALENDAR_SUMMARY).await {
            Ok(calendar) => calendar,
            Err(error) => {
                return record_connect_error(
                    state,
                    connection_id,
                    &format!("the Study Plan calendar could not be created: {error}"),
                )
                .await;
            }
        },
    };

    // An id-less answer cannot be stored: `study_calendar_id` is what every later pass lists,
    // and `''` would be a calendar id that is not one.
    if calendar.id.is_empty() {
        return record_connect_error(state, connection_id, "the Study Plan calendar has no id")
            .await;
    }

    sqlx::query(
        "update calendar_connections
            set study_calendar_id = $2,
                error = null,
                updated_at = $3
          where id = $1",
    )
    .bind(connection_id)
    .bind(&calendar.id)
    .bind(state.clock.now())
    .execute(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("storing the study calendar: {error}")))?;

    tracing::info!(%connection_id, calendar_id = %calendar.id, "the Study Plan calendar is ready");
    Ok(())
}

/// Records a connect-time reason on the connection and leaves it `connected`.
///
/// Not [`crate::calendar::sync::record_failure`]: that one marks the connection `error` and
/// drops the ICS validators, and neither is true here — the grant is live and the sync still
/// imports. What is missing is the mirror destination, and A247's "the reason is shown beside
/// the connection" is the whole remedy. The next clean sync clears it, which is what keeps the
/// field honest rather than sticky.
async fn record_connect_error(
    state: &AppState,
    connection_id: Uuid,
    reason: &str,
) -> AppResult<()> {
    sqlx::query("update calendar_connections set error = $2, updated_at = $3 where id = $1")
        .bind(connection_id)
        .bind(reason)
        .bind(state.clock.now())
        .execute(&state.pool)
        .await
        .map_err(|error| {
            AppError::Internal(anyhow::anyhow!("recording a connect error: {error}"))
        })?;

    tracing::warn!(%connection_id, %reason, "the Study Plan calendar could not be resolved");
    Ok(())
}

/// The plaintext of a stored grant.
///
/// The inverse of [`crate::auth::oauth_state::encrypt_secret`], and deliberately its mirror:
/// the format is `nonce || ciphertext` with the key from `ENCRYPTION_KEY_HEX`, and the only
/// reader that matters is the sync. A row that does not decrypt is a row this build cannot use
/// — a rotated key, or a truncated write — so the error is a reason to record rather than to
/// retry.
pub fn decrypt_refresh_token(state: &AppState, encrypted: &[u8]) -> AppResult<String> {
    let unusable = |reason: &str| AppError::Internal(anyhow::anyhow!("{reason}"));

    // 12 bytes of nonce plus at least a tag: anything shorter cannot be a ciphertext.
    if encrypted.len() < 13 {
        return Err(unusable(
            "a stored calendar grant is too short to be a ciphertext",
        ));
    }
    let (nonce_bytes, ciphertext) = encrypted.split_at(12);
    let nonce: [u8; 12] = nonce_bytes
        .try_into()
        .map_err(|_| unusable("a stored calendar grant has no nonce"))?;

    let plaintext = crate::auth::oauth_state::cipher(state)?
        .decrypt(&Nonce::from(nonce), ciphertext)
        .map_err(|_| unusable("a stored calendar grant did not decrypt"))?;

    String::from_utf8(plaintext).map_err(|_| unusable("a stored calendar grant is not text"))
}

/// A redirect to the app's deep link, with the reason percent-encoded.
///
/// Built through `url::Url` rather than by `format!`: a reason carrying a space, an `&` or a
/// non-ASCII character would otherwise produce a malformed URL, and a malformed deep link is
/// a silent no-op at the OS handler — the student would be returned to the app with nothing
/// said at all.
///
/// **302, not axum's `Redirect::to`.** That helper answers 303, and the login flow's callback
/// answers 302; two OAuth returns in one product disagreeing about a status a client may
/// branch on is the kind of difference that is invisible until one of them breaks. This is
/// the same construction `auth::routes::google_callback` uses.
///
/// `AppResult` rather than a bare `Response` because the workspace denies `expect`, and a
/// `.expect("this literal parses")` on a constant is exactly the pattern that rule exists to
/// keep out — a `url::Url` error is unreachable here, and mapping it is what makes that
/// visible rather than asserted.
fn deep_link(status: &str, reason: Option<&str>) -> AppResult<axum::response::Response> {
    let mut params: Vec<(&str, &str)> = vec![("status", status)];
    if let Some(reason) = reason {
        params.push(("reason", reason));
    }
    let url = url::Url::parse_with_params(DEEP_LINK, &params)
        .map_err(|error| AppError::Internal(anyhow::anyhow!("building the deep link: {error}")))?;
    Ok((
        axum::http::StatusCode::FOUND,
        [(axum::http::header::LOCATION, url.to_string())],
    )
        .into_response())
}

/// Finds the student's Google connection and refreshes its grant, or creates one.
///
/// A re-connect **updates** rather than inserting a second row. `calendar_connections` has no
/// unique index on `(user_id, provider)` on purpose — two Google accounts for one student are
/// two rows, and the cap is the engine's business rather than a constraint's (A245) — so the
/// update is an explicit read-then-write rather than an `on conflict`.
///
/// Without it, a student who reconnects after revoking the grant would end up with two rows
/// for one calendar, and both would import every event: `events`' uniqueness is on
/// `(connection_id, external_id)`, which two connections do not share.
///
/// `account_email` stays `null`. The two scopes this flow asks for carry no email claim, and
/// inventing one from the login session would be a fact this row cannot support — the column
/// is nullable and the panel renders it as absent.
///
/// Returns the row's id, because the caller's next step — [`ensure_study_calendar`] — writes
/// `study_calendar_id` onto it.
async fn store_connection(
    state: &AppState,
    user_id: Uuid,
    refresh_token_enc: &[u8],
) -> AppResult<Uuid> {
    let existing: Option<Uuid> = sqlx::query_scalar(
        "select id from calendar_connections
          where user_id = $1 and provider = 'google'
          order by created_at asc, id asc
          limit 1",
    )
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| {
        AppError::Internal(anyhow::anyhow!("reading the Google connection: {error}"))
    })?;

    let now = state.clock.now();
    let id = match existing {
        Some(id) => {
            sqlx::query(
                "update calendar_connections
                    set refresh_token_enc = $2,
                        status = 'connected',
                        error = null,
                        updated_at = $3
                  where id = $1",
            )
            .bind(id)
            .bind(refresh_token_enc)
            .bind(now)
            .execute(&state.pool)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("refreshing the Google connection: {error}"))
            })?;
            id
        }
        None => {
            let id: Uuid = sqlx::query_scalar(
                "insert into calendar_connections
                    (user_id, provider, refresh_token_enc, status, updated_at)
                 values ($1, 'google', $2, 'connected', $3)
                 returning id",
            )
            .bind(user_id)
            .bind(refresh_token_enc)
            .bind(now)
            .fetch_one(&state.pool)
            .await
            .map_err(|error| {
                AppError::Internal(anyhow::anyhow!("writing the Google connection: {error}"))
            })?;
            id
        }
    };
    Ok(id)
}

/// Exchanges the authorization code, with the state's own PKCE verifier.
///
/// The same four form fields `auth::google::exchange_code` sends, and the same shape of
/// error: `error_description` when Google supplies one, because
/// `redirect_uri_mismatch` versus `invalid_grant` is the difference between a five-minute
/// fix and an afternoon.
///
/// This is the **only** call in this module that reaches Google, and it is therefore the
/// boundary of what this build can test.
async fn exchange_code(
    google: &crate::config::GoogleCalendarConfig,
    code: &str,
    verifier: &str,
) -> AppResult<Value> {
    let client = reqwest::Client::new();
    let response = client
        .post(GOOGLE_TOKEN_ENDPOINT)
        .form(&[
            ("client_id", google.client_id.as_str()),
            ("client_secret", google.client_secret.expose_secret()),
            ("code", code),
            ("code_verifier", verifier),
            ("grant_type", "authorization_code"),
            ("redirect_uri", google.redirect_uri.as_str()),
        ])
        .send()
        .await
        .map_err(|error| AppError::Provider(format!("Google token exchange: {error}")))?;

    let status = response.status();
    let body: Value = response
        .json()
        .await
        .map_err(|error| AppError::Provider(format!("Google token response: {error}")))?;

    if !status.is_success() {
        let reason = body
            .get("error_description")
            .or_else(|| body.get("error"))
            .and_then(Value::as_str)
            .unwrap_or("unknown error");
        return Err(AppError::Provider(format!(
            "Google rejected the calendar code exchange: {reason}"
        )));
    }
    Ok(body)
}
