//! The shell's sync: the pass the native app runs, and the surface the SPA drives.
//!
//! The engine lives in `crates/sync` and is generic over a store and a transport (A182);
//! this module is the device side of it. It owns three things the engine deliberately does
//! not know about:
//!
//!   - **The credentials are pushed, never stored.** `sync_configure` puts the API origin,
//!     the access token and this device's id in memory. The token is never written to
//!     SQLite — the keychain is the only place a token may live — and the cache holds data,
//!     not credentials.
//!   - **The events the SPA listens for.** `stvdy://sync-status` carries every report,
//!     `stvdy://sync-revoked` says the cache and the outbox are gone (A186), and
//!     `stvdy://sync-auth-expired` says the pass stopped because the access token expired,
//!     which the SPA answers with a refresh and a reconfigure.
//!   - **Whose cache this is.** A session binds the cache to the account its token names;
//!     a different account wipes first, so a pending offline write from one account is never
//!     replayed by the next one that signs in on this machine.

pub mod commands;
pub mod transport;

use std::sync::{Arc, Mutex, MutexGuard};

use chrono::{DateTime, Utc};
use serde::Serialize;
use serde_json::Value;
use stvdy_sync::client::{Engine, LocalStore, StoreError, SyncState, Transport, TransportError};
use stvdy_sync::proto::{Entity, PullQuery, PullResponse, PushRequest, PushResponse};
use uuid::Uuid;

use crate::cache::SqliteStore;
use crate::keychain;
use transport::HttpTransport;

/// Emitted with a [`SyncReport`] whenever a pass ends, a local write queues work, or the
/// cache is reset.
pub const SYNC_STATUS_EVENT: &str = "stvdy://sync-status";

/// Emitted once, after the shell has wiped the cache and the outbox (A186). The SPA signs
/// out on it.
pub const SYNC_REVOKED_EVENT: &str = "stvdy://sync-revoked";

/// Emitted when the access token expired. Nothing is wiped: the SPA refreshes, reconfigures
/// and runs one pass.
pub const SYNC_AUTH_EXPIRED_EVENT: &str = "stvdy://sync-auth-expired";

/// The key the SPA persists the refresh token under (`app/src/lib/tokens.ts`). A revoked
/// device's token is deleted here too, because A186 names the shell as well as the SPA.
pub const REFRESH_TOKEN_KEY: &str = "stvdy.refresh_token";

/// What the shell tells the app, in the three shapes the SPA listens for.
#[derive(Debug, Clone)]
pub enum SyncNotice {
    Status(SyncReport),
    Revoked,
    AuthExpired,
}

/// The shell's way of reaching the app. The transport emits `AuthExpired` from inside a
/// pass and the shell emits the other two, so both are handed the same closure.
pub type Notifier = Arc<dyn Fn(SyncNotice) + Send + Sync>;

/// What the app shows about sync (A187): the engine's state, the server's code when one is
/// owed, the stored last-sync time and the outbox length.
#[derive(Debug, Clone, PartialEq, Serialize)]
pub struct SyncReport {
    /// `synced | syncing | offline | needs_attention | revoked`.
    pub state: &'static str,
    /// The server's own code, for `needs_attention` only.
    pub code: Option<String>,
    /// The device's own clock at the end of the last pass, or `null` before the first one.
    /// Never a server time and never a fresh clock read (gate 7(a)).
    pub last_synced_at: Option<DateTime<Utc>>,
    /// How many mutations are still queued. `synced` with a non-zero outbox is a state the
    /// engine cannot produce.
    pub outbox: usize,
}

impl SyncReport {
    /// The report a revoked device gets: everything local is gone.
    fn revoked() -> Self {
        Self {
            state: "revoked",
            code: None,
            last_synced_at: None,
            outbox: 0,
        }
    }
}

/// The credentials `sync_configure` hands the shell.
pub struct Credentials {
    pub base_url: String,
    pub access_token: String,
}

/// How the shell builds a transport for a session.
///
/// A seam rather than a type parameter, because the credentials arrive at runtime and the
/// engine wants a transport *value*: the shell's own factory is HTTP, and the shell's test
/// suite hands it a scripted double so a pass can be driven without a server — the same
/// double the convergence suite drives the real routes with.
pub type TransportFactory =
    Arc<dyn Fn(&Credentials) -> Result<DynTransport, TransportError> + Send + Sync>;

/// A transport behind an `Arc`, so the shell can pick one when a session is configured.
///
/// `stvdy-sync`'s trait is not implemented for `Arc<dyn Transport>` there (both types are
/// foreign to this crate), so the newtype is this crate's.
#[derive(Clone)]
pub struct DynTransport(Arc<dyn Transport>);

impl DynTransport {
    pub fn new(transport: impl Transport + 'static) -> Self {
        Self(Arc::new(transport))
    }
}

#[async_trait::async_trait]
impl Transport for DynTransport {
    async fn pull(&self, query: PullQuery) -> Result<PullResponse, TransportError> {
        self.0.pull(query).await
    }

    async fn push(&self, request: PushRequest) -> Result<PushResponse, TransportError> {
        self.0.push(request).await
    }
}

/// The transport of a cache with no session: every pass fails as if the network were down,
/// which is the engine's own word for "nothing was reached, waiting is the remedy".
pub struct Unconfigured;

#[async_trait::async_trait]
impl Transport for Unconfigured {
    async fn pull(&self, _query: PullQuery) -> Result<PullResponse, TransportError> {
        Err(TransportError::Network(
            "sync is not configured: the app has no session".to_string(),
        ))
    }

    async fn push(&self, _request: PushRequest) -> Result<PushResponse, TransportError> {
        Err(TransportError::Network(
            "sync is not configured: the app has no session".to_string(),
        ))
    }
}

/// A shell failure the SPA is shown.
#[derive(Debug, thiserror::Error)]
pub enum SyncError {
    #[error("the local cache failed: {0}")]
    Store(#[from] StoreError),
    #[error("the sync transport failed: {0}")]
    Transport(#[from] TransportError),
}

/// The native app's sync: a cache, a session, and the pass.
pub struct SyncShell {
    cache: SqliteStore,
    session: Mutex<Option<Session>>,
    notifier: Notifier,
    factory: TransportFactory,
}

#[derive(Clone)]
struct Session {
    device_id: Uuid,
    transport: DynTransport,
}

impl SyncShell {
    /// The shell the app runs, over HTTP.
    pub fn new(cache: SqliteStore, notifier: Notifier) -> Self {
        let http = notifier.clone();
        Self::with_factory(
            cache,
            Arc::new(move |credentials| {
                HttpTransport::new(
                    &credentials.base_url,
                    &credentials.access_token,
                    http.clone(),
                )
                .map(DynTransport::new)
            }),
            notifier,
        )
    }

    /// The same shell with a different transport factory, for a test that drives a pass
    /// without a server.
    pub fn with_factory(cache: SqliteStore, factory: TransportFactory, notifier: Notifier) -> Self {
        Self {
            cache,
            session: Mutex::new(None),
            notifier,
            factory,
        }
    }

    /// Takes the credentials of the current session.
    ///
    /// Called on boot and again after every refresh, because the access token is
    /// short-lived. The account the token names is what binds the cache: if it differs from
    /// the account the file already belongs to, the file is wiped first — the cross-account
    /// write this exists to prevent.
    pub fn configure(
        &self,
        base_url: &str,
        access_token: &str,
        device_id: Uuid,
    ) -> Result<(), SyncError> {
        if let Some(account) = account_of(access_token) {
            self.cache.bind(account)?;
        }
        let transport = (self.factory)(&Credentials {
            base_url: base_url.to_string(),
            access_token: access_token.to_string(),
        })?;
        *self.lock_session() = Some(Session {
            device_id,
            transport,
        });
        tracing::info!(base_url, "sync configured");
        Ok(())
    }

    /// The status, from stored facts only: the engine's own rule, over this cache.
    pub fn status(&self) -> Result<SyncReport, SyncError> {
        let session = self.session();
        let device_id = session
            .as_ref()
            .map(|session| session.device_id)
            .unwrap_or_else(Uuid::nil);
        let transport = session
            .map(|session| session.transport)
            .unwrap_or_else(|| DynTransport::new(Unconfigured));
        let engine = Engine::new(self.cache.clone(), transport, device_id);
        self.report(engine.status()?)
    }

    /// One pass. The outbox first, then every page, then the facts the status reads.
    pub async fn now(&self) -> Result<SyncReport, SyncError> {
        self.pass(true).await
    }

    /// The pass itself, with the notifier made optional.
    ///
    /// `announce` is `false` for exactly one caller: the background scheduler (A315).
    /// The notifier is how the shell reaches the SPA — `sync::commands::emit` turns a
    /// [`SyncNotice`] into a Tauri event — and a background launch has no listener: the
    /// webview may not exist, and if it does it is suspended and will not hear an event
    /// that was never queued. Emitting there would be a report nobody can see, and the
    /// property A315 asks for is stronger than "harmless" — the background entry points
    /// emit no frontend event at all. The pass's *result* is returned either way, which
    /// is what the scheduler logs.
    ///
    /// `forget` still runs on a revoked device, because that is not a notification: it
    /// drops the session and the refresh token, and a background pass that discovered a
    /// revocation and left the credentials in place would be one that made the next
    /// foreground pass fail instead of sign out.
    async fn pass(&self, announce: bool) -> Result<SyncReport, SyncError> {
        let Some(session) = self.session() else {
            // Nothing was reached because there is nothing to reach with; the engine's word
            // for that is `offline`, and waiting is the remedy once a session arrives.
            return self.report(SyncState::Offline);
        };
        let mut engine = Engine::new(self.cache.clone(), session.transport, session.device_id);
        let state = engine.sync_once().await?;
        if state == SyncState::Revoked {
            self.forget();
            if announce {
                (self.notifier)(SyncNotice::Revoked);
            }
            return Ok(SyncReport::revoked());
        }
        let report = self.report(state)?;
        if announce {
            (self.notifier)(SyncNotice::Status(report.clone()));
        }
        Ok(report)
    }

    /// One pass, for a caller that has no webview (A315).
    ///
    /// This is the function the native background schedulers call — the iOS
    /// `BGAppRefreshTask`/`BGProcessingTask` handlers and the Android `WorkManager`
    /// worker — and it is deliberately the *same* pass `sync_now` runs rather than a
    /// second path: the engine's outbox drain, its pull, its conflict handling and its
    /// revocation rule all live in `stvdy_sync::client::Engine::sync_once`, and a
    /// background sync that reimplemented any of that would be a second answer to
    /// "what does a pass do".
    ///
    /// **It is called with no frontend, and that shapes what it can promise.** A
    /// background launch may run before the SPA ever has, and the shell's credentials
    /// are pushed by the SPA and never stored (this module's own rule: the keychain
    /// holds a refresh token, not an access token), so a pass with no session reports
    /// `offline` — the engine's word for "nothing was reached, waiting is the remedy" —
    /// rather than failing. That is A316: a background window is opportunistic and the
    /// next foreground sync is what makes the device correct.
    pub async fn sync_once(&self) -> Result<SyncReport, SyncError> {
        self.pass(false).await
    }

    /// Empties the cache and the outbox, then rebuilds from zero: "Reset local data and
    /// resync".
    pub async fn reset(&self) -> Result<SyncReport, SyncError> {
        let mut cache = self.cache.clone();
        cache.wipe()?;
        let report = self.pass(true).await?;
        (self.notifier)(SyncNotice::Status(report.clone()));
        Ok(report)
    }

    /// The cached row, or the entity's rows.
    pub fn read(&self, entity: Entity, id: Option<Uuid>) -> Result<Value, SyncError> {
        Ok(self.cache.read(entity, id)?)
    }

    /// The local-first write: the row lands in the cache and its mutation in the outbox.
    pub fn write(&self, entity: Entity, id: Uuid, payload: &Value) -> Result<(), SyncError> {
        self.cache.write(entity, id, payload)?;
        self.announce()
    }

    /// The local-first tombstone.
    pub fn tombstone(&self, entity: Entity, id: Uuid) -> Result<(), SyncError> {
        self.cache.tombstone(entity, id)?;
        self.announce()
    }

    /// The app's own state for `key`: a UI preference or a running timer's marker, which
    /// belongs to the device rather than to the account and is not wiped with the cache.
    pub fn local_state_get(&self, key: &str) -> Result<Option<String>, SyncError> {
        Ok(self
            .cache
            .with_connection(|conn| crate::cache::local_state_get(conn, key))
            .map_err(StoreError::backend)?)
    }

    /// Sets or clears the app's own state for `key`. `None` deletes the row.
    pub fn local_state_set(&self, key: &str, value: Option<&str>) -> Result<(), SyncError> {
        self.cache
            .with_connection(|conn| crate::cache::local_state_set(conn, key, value))
            .map_err(StoreError::backend)?;
        Ok(())
    }

    /// A revoked device keeps no session: the refresh token goes with the cache, and the
    /// next pass cannot run against a session the server has already refused.
    fn forget(&self) {
        *self.lock_session() = None;
        if let Err(error) = keychain::delete(REFRESH_TOKEN_KEY) {
            tracing::warn!(%error, "could not delete the refresh token of a revoked device");
        }
    }

    /// Tells the app what the queue now holds, so a status control cannot keep showing
    /// `synced` after a local write (A187).
    fn announce(&self) -> Result<(), SyncError> {
        let report = self.status()?;
        (self.notifier)(SyncNotice::Status(report));
        Ok(())
    }

    fn report(&self, state: SyncState) -> Result<SyncReport, SyncError> {
        // The words are the wire's, and the SPA's `SyncStatus` union is built from them.
        let (word, code) = match state {
            SyncState::Synced => ("synced", None),
            SyncState::Syncing => ("syncing", None),
            SyncState::Offline => ("offline", None),
            SyncState::NeedsAttention { code } => ("needs_attention", Some(code)),
            SyncState::Revoked => ("revoked", None),
        };
        Ok(SyncReport {
            state: word,
            code,
            last_synced_at: self.cache.last_synced_at()?,
            outbox: self.cache.outbox_len()?,
        })
    }

    fn session(&self) -> Option<Session> {
        self.lock_session().clone()
    }

    fn lock_session(&self) -> MutexGuard<'_, Option<Session>> {
        self.session
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

/// The account an access token names, from its own `sub` claim.
///
/// This is a **local** decision — whose data this cache file holds, and whether the previous
/// account's must be wiped before the next one's rows arrive — and never an authorization
/// one: the server authorizes every request, and this shell trusts nothing it reads out of a
/// token. A token it cannot read simply leaves the cache unbound, which is safe in both
/// directions: an unbound cache holds no rows for an account it cannot name, and binding
/// later wipes whatever is there.
fn account_of(access_token: &str) -> Option<Uuid> {
    use base64::Engine;

    let payload = access_token.split('.').nth(1)?;
    let decoded = base64::engine::general_purpose::URL_SAFE_NO_PAD
        .decode(payload)
        .ok()?;
    let claims: Value = serde_json::from_slice(&decoded).ok()?;
    Uuid::parse_str(claims.get("sub")?.as_str()?).ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use base64::Engine;

    fn token(claims: &str) -> String {
        let encode = |bytes: &[u8]| base64::engine::general_purpose::URL_SAFE_NO_PAD.encode(bytes);
        format!(
            "{}.{}.{}",
            encode(b"{}"),
            encode(claims.as_bytes()),
            encode(b"sig")
        )
    }

    /// The account comes from the token's own `sub`, and a token the shell cannot read
    /// binds nothing — which is the safe direction: an unbound cache holds no rows for an
    /// account it cannot name, and binding one later wipes whatever is there.
    #[test]
    fn only_a_readable_token_binds_the_cache_to_an_account() {
        let account = Uuid::now_v7();
        assert_eq!(
            account_of(&token(&format!(r#"{{"sub":"{account}","did":"d"}}"#))),
            Some(account)
        );

        for unreadable in [
            // Not three parts, so not a JWT.
            "an-opaque-token".to_string(),
            // A payload that is not base64.
            "header.!!!not base64!!!.sig".to_string(),
            // A payload that is base64 but not JSON.
            token("not json"),
            // No `sub`.
            token(r#"{"did":"d"}"#),
            // A `sub` that is not a uuid: the server would never issue one, so this is a
            // token from somewhere else and it must not decide whose cache this is.
            token(r#"{"sub":"somebody-else"}"#),
        ] {
            assert_eq!(account_of(&unreadable), None, "read {unreadable}");
        }
    }

    /// The wire shape the SPA reads: `state`, `code`, `last_synced_at`, `outbox`. These
    /// names are `app/src/platform/desktop.ts`'s `WireReport`, and a rename here would be a
    /// runtime failure in the bundled app that no other test in this crate would see.
    #[test]
    fn the_report_serializes_to_the_shape_the_spa_reads() {
        let at = DateTime::parse_from_rfc3339("2026-10-12T03:04:05Z")
            .expect("the fixture parses")
            .with_timezone(&Utc);
        let report = SyncReport {
            state: "needs_attention",
            code: Some("read_only_entity".to_string()),
            last_synced_at: Some(at),
            outbox: 2,
        };
        let value = serde_json::to_value(&report).expect("the report serializes");
        assert_eq!(
            value,
            serde_json::json!({
                "state": "needs_attention",
                "code": "read_only_entity",
                "last_synced_at": "2026-10-12T03:04:05Z",
                "outbox": 2,
            })
        );

        let fresh = SyncReport {
            state: "syncing",
            code: None,
            last_synced_at: None,
            outbox: 0,
        };
        assert_eq!(
            serde_json::to_value(&fresh).expect("the report serializes"),
            serde_json::json!({
                "state": "syncing",
                "code": null,
                "last_synced_at": null,
                "outbox": 0,
            })
        );
    }
}
