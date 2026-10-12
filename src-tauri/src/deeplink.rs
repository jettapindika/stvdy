//! Classification of incoming `stvdy://` URLs.
//!
//! The shell, not the SPA, decides what an incoming URL means. The frontend
//! receives a serialized [`DeepLinkIntent`] (`{"action": …, "payload": …}`)
//! rather than a raw URL string, so a route the app does not understand is
//! rejected in one place and a malformed URL can never reach a handler.
//!
//! This module is pure: no IO, no logging, no panics. A URL query string can
//! carry an OAuth `code` or a login `ticket` (both credentials), so nothing here
//! writes a URL anywhere; callers log [`DeepLinkIntent::action`] only.

use std::collections::BTreeMap;

const SCHEME: &str = "stvdy://";

/// What the app must do with an incoming URL.
///
/// Serialized as `{"action": "<variant>", "payload": {…}}` with snake_case
/// variant names, which is the shape `app/src/platform/types.ts` declares.
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
#[serde(tag = "action", content = "payload", rename_all = "snake_case")]
pub enum DeepLinkIntent {
    /// A login/OAuth callback. Carries the `code` and `state` the SPA exchanges
    /// for a token pair.
    AuthCallback(AuthCallback),
    /// A Google sign-in return. Carries the single-use login `ticket` the SPA
    /// exchanges for a token pair. Deliberately not a token pair: the server
    /// never puts a long-lived credential in a URL the browser or OS handler
    /// can see, only a ticket the app redeems once.
    AuthTicket(AuthTicket),
    /// A calendar OAuth return. Carries the query parameters verbatim.
    Calendar(Calendar),
    /// Anything else, with the reason so the shell can log *why* without
    /// logging the URL.
    Ignored(Ignored),
}

impl DeepLinkIntent {
    /// The action name, safe to log. Never includes the URL or its query.
    pub fn action(&self) -> &'static str {
        match self {
            DeepLinkIntent::AuthCallback(_) => "auth_callback",
            DeepLinkIntent::AuthTicket(_) => "auth_ticket",
            DeepLinkIntent::Calendar(_) => "calendar",
            DeepLinkIntent::Ignored(_) => "ignored",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthCallback {
    pub code: String,
    pub state: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct AuthTicket {
    pub ticket: String,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Calendar {
    pub params: BTreeMap<String, String>,
}

#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct Ignored {
    pub reason: IgnoreReason,
}

/// Why a URL was not actionable. Deliberately coarse: it must not echo any part
/// of the URL back into a log line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize)]
#[serde(rename_all = "snake_case")]
pub enum IgnoreReason {
    /// The scheme is not `stvdy://` (for example an `https://` link).
    NotStvdyScheme,
    /// A universal link (`https://`) whose host is not the one this app claims, or one
    /// on a transport the OS did not verify (`http://`). Separate from
    /// [`IgnoreReason::NotStvdyScheme`] because it is a *different* URL that arrived
    /// by a route the app does advertise, which is the one worth seeing in a log.
    NotOurHost,
    /// The scheme is ours but the route is not one this app handles.
    UnknownRoute,
    /// `auth/callback` with a missing or blank `code`.
    MissingCode,
    /// `auth/callback` with a missing or blank `state`.
    MissingState,
    /// `auth/callback` with no usable credential: a blank `ticket`, or no
    /// `ticket`/`code`/`state` parameter at all.
    MissingTicket,
}

/// Classifies one incoming URL. Never panics: any input that is not a valid,
/// actionable URL of ours becomes [`DeepLinkIntent::Ignored`].
///
/// **Two schemes, one vocabulary (A320).** A URL arrives either as `stvdy://<route>`
/// (the custom scheme, registered on both platforms) or as `https://app.stvdy/<route>`
/// (a universal link on iOS, an app link on Android). Both are reduced to the same
/// route-and-query pair *before* the route match below, so there is one handler table
/// and one set of tests for both — a second parsing path would be a second vocabulary,
/// and only one of the two would ever be exercised.
pub fn classify(raw: &str) -> DeepLinkIntent {
    let rest = match strip_scheme(raw) {
        Ok(rest) => rest,
        Err(reason) => return DeepLinkIntent::Ignored(Ignored { reason }),
    };

    // A fragment is never meaningful to an OAuth return; drop it before parsing.
    let rest = rest.split_once('#').map_or(rest, |(before, _)| before);
    let (path, query) = rest
        .split_once('?')
        .map_or((rest, ""), |(path, query)| (path, query));

    let route = path.trim_matches('/').to_ascii_lowercase();
    let params = parse_query(query);

    match route.as_str() {
        "auth/callback" => auth_callback(&params),
        "oauth/calendar" => DeepLinkIntent::Calendar(Calendar { params }),
        _ => DeepLinkIntent::Ignored(Ignored {
            reason: IgnoreReason::UnknownRoute,
        }),
    }
}

/// Classifies the `auth/callback` route, which accepts either credential pair:
/// a complete `code`+`state` (the Calendar OAuth return) or a `ticket` (the
/// Google sign-in return). A complete `code`+`state` pair keeps precedence, so
/// a URL that already classified as an OAuth callback still does.
fn auth_callback(params: &BTreeMap<String, String>) -> DeepLinkIntent {
    let code = non_blank(params.get("code"));
    let state = non_blank(params.get("state"));

    match (code, state) {
        (Some(code), Some(state)) => DeepLinkIntent::AuthCallback(AuthCallback { code, state }),
        (code, _) => {
            if let Some(ticket) = non_blank(params.get("ticket")) {
                return DeepLinkIntent::AuthTicket(AuthTicket { ticket });
            }

            // No usable credential. Report the ticket reason only when the URL
            // carried no OAuth parameters at all; otherwise preserve the
            // incomplete-pair reason.
            if !params.contains_key("code") && !params.contains_key("state") {
                return DeepLinkIntent::Ignored(Ignored {
                    reason: IgnoreReason::MissingTicket,
                });
            }

            match code {
                None => DeepLinkIntent::Ignored(Ignored {
                    reason: IgnoreReason::MissingCode,
                }),
                Some(_) => DeepLinkIntent::Ignored(Ignored {
                    reason: IgnoreReason::MissingState,
                }),
            }
        }
    }
}

/// A parameter counts as present only if it has a non-whitespace value. `code=`
/// and `code=%20` are both "missing", not a blank credential handed to a handler.
fn non_blank(value: Option<&String>) -> Option<String> {
    let value = value?.trim();
    if value.is_empty() {
        None
    } else {
        Some(value.to_string())
    }
}

/// The host a universal link must carry to be ours. **The same string
/// `plugins.deep-link.mobile[].host` names**, which is what the CLI turns into
/// `applinks:app.stvdy` in the app's entitlements — asserted by
/// `tests/deep_link.rs` against `tauri.conf.json` rather than trusted to match.
pub const APP_LINK_HOST: &str = "app.stvdy";

/// Reduces a URL of either scheme to its route and query, or says why it is not ours.
///
/// `https://app.stvdy/auth/callback?code=…` and `stvdy://auth/callback?code=…` both
/// yield `auth/callback?code=…`. Everything else — a foreign host, `http://`, a
/// different custom scheme, a userinfo section — is refused, because the only thing
/// that makes a URL actionable is that the OS verified the app as its owner.
fn strip_scheme(raw: &str) -> Result<&str, IgnoreReason> {
    if let Some(rest) = strip_prefix_ignore_case(raw, SCHEME) {
        return Ok(rest);
    }

    // A universal link. `https` only: the AASA file is fetched over TLS and the OS
    // verifies the association, so an `http://` URL that reached here is a link that
    // could have been forged in transit, and it is refused rather than downgraded.
    let Some(rest) = strip_prefix_ignore_case(raw, "https://") else {
        return Err(IgnoreReason::NotStvdyScheme);
    };
    // No userinfo: `https://app.stvdy@evil.example/…` is a URL whose host is
    // `evil.example`, and reading the authority as a whole would accept it.
    let authority_end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..authority_end];
    // The port, if any, is dropped before the host is compared: `app.stvdy:443` is the
    // same host as `app.stvdy`, and a link that classified differently because a caller
    // wrote the default port would be a route that works from one client and not another.
    let host = authority
        .split('@')
        .next_back()
        .unwrap_or(authority)
        .split(':')
        .next()
        .unwrap_or(authority);
    if authority.contains('@') || !host.eq_ignore_ascii_case(APP_LINK_HOST) {
        return Err(IgnoreReason::NotOurHost);
    }
    rest.get(authority_end..).ok_or(IgnoreReason::NotOurHost)
}

/// `str::strip_prefix` is case-sensitive and a URL's scheme is not.
fn strip_prefix_ignore_case<'a>(raw: &'a str, prefix: &str) -> Option<&'a str> {
    let head = raw.get(..prefix.len())?;
    if head.eq_ignore_ascii_case(prefix) {
        raw.get(prefix.len()..)
    } else {
        None
    }
}

fn parse_query(query: &str) -> BTreeMap<String, String> {
    let mut params = BTreeMap::new();
    for pair in query.split('&') {
        if pair.is_empty() {
            continue;
        }
        let (key, value) = pair
            .split_once('=')
            .map_or((pair, ""), |(key, value)| (key, value));
        if key.is_empty() {
            continue;
        }
        params.insert(percent_decode(key), percent_decode(value));
    }
    params
}

/// Decodes `%XX` escapes. An invalid escape is left literal rather than dropped,
/// so a malformed URL degrades instead of losing bytes. `+` is left alone: an
/// OAuth code is percent-encoded, not form-encoded.
fn percent_decode(input: &str) -> String {
    let bytes = input.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut index = 0;
    while index < bytes.len() {
        if bytes[index] == b'%'
            && index + 2 < bytes.len()
            && let (Some(high), Some(low)) =
                (hex_value(bytes[index + 1]), hex_value(bytes[index + 2]))
        {
            out.push(high * 16 + low);
            index += 3;
            continue;
        }
        out.push(bytes[index]);
        index += 1;
    }
    String::from_utf8_lossy(&out).into_owned()
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn classifies_an_auth_callback() {
        let intent = classify("stvdy://auth/callback?code=abc123&state=xyz");
        match intent {
            DeepLinkIntent::AuthCallback(AuthCallback { code, state }) => {
                assert_eq!(code, "abc123");
                assert_eq!(state, "xyz");
            }
            other => panic!("expected an auth callback, got {other:?}"),
        }
    }

    #[test]
    fn percent_decodes_the_code_and_state() {
        let intent = classify("stvdy://auth/callback?code=a%2Fb%2Bc&state=s%20t");
        match intent {
            DeepLinkIntent::AuthCallback(AuthCallback { code, state }) => {
                assert_eq!(code, "a/b+c");
                assert_eq!(state, "s t");
            }
            other => panic!("expected an auth callback, got {other:?}"),
        }
    }

    #[test]
    fn classifies_an_auth_ticket() {
        let intent = classify("stvdy://auth/callback?ticket=one-time-ticket");
        match intent {
            DeepLinkIntent::AuthTicket(AuthTicket { ticket }) => {
                assert_eq!(ticket, "one-time-ticket");
            }
            other => panic!("expected an auth ticket, got {other:?}"),
        }
    }

    #[test]
    fn percent_decodes_the_ticket() {
        let intent = classify("stvdy://auth/callback?ticket=a%2Fb%2Bc");
        match intent {
            DeepLinkIntent::AuthTicket(AuthTicket { ticket }) => {
                assert_eq!(ticket, "a/b+c");
            }
            other => panic!("expected an auth ticket, got {other:?}"),
        }
    }

    #[test]
    fn blank_or_absent_ticket_is_ignored_with_a_reason() {
        for url in [
            "stvdy://auth/callback?ticket=",
            "stvdy://auth/callback?ticket=%20",
            "stvdy://auth/callback",
        ] {
            assert_eq!(
                classify(url),
                DeepLinkIntent::Ignored(Ignored {
                    reason: IgnoreReason::MissingTicket
                }),
                "expected {url} to be ignored as a missing ticket"
            );
        }
    }

    #[test]
    fn a_complete_code_and_state_pair_still_classifies_as_a_callback() {
        // The Calendar OAuth return path must keep working unchanged, even with
        // a stray blank ticket parameter alongside it.
        let intent = classify("stvdy://auth/callback?code=abc123&state=xyz&ticket=");
        match intent {
            DeepLinkIntent::AuthCallback(AuthCallback { code, state }) => {
                assert_eq!(code, "abc123");
                assert_eq!(state, "xyz");
            }
            other => panic!("expected an auth callback, got {other:?}"),
        }
    }

    #[test]
    fn a_ticket_never_reaches_the_loggable_action_label() {
        let intent = classify("stvdy://auth/callback?ticket=SUPERSECRET");
        // The label callers log carries no credential.
        assert_eq!(intent.action(), "auth_ticket");
        assert!(!intent.action().contains("SUPERSECRET"));

        // The serialized intent carries the ticket only inside the payload, and
        // the action label stays the fixed `auth_ticket` string.
        let value = serde_json::to_value(&intent).expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({
                "action": "auth_ticket",
                "payload": { "ticket": "SUPERSECRET" }
            })
        );
    }

    #[test]
    fn classifies_a_calendar_return() {
        let intent = classify("stvdy://oauth/calendar?provider=google&state=abc");
        match intent {
            DeepLinkIntent::Calendar(Calendar { params }) => {
                assert_eq!(params.get("provider").map(String::as_str), Some("google"));
                assert_eq!(params.get("state").map(String::as_str), Some("abc"));
            }
            other => panic!("expected a calendar action, got {other:?}"),
        }
    }

    #[test]
    fn missing_or_blank_credentials_are_ignored_with_a_reason() {
        let missing_code = classify("stvdy://auth/callback?state=xyz");
        assert_eq!(
            missing_code,
            DeepLinkIntent::Ignored(Ignored {
                reason: IgnoreReason::MissingCode
            })
        );

        let blank_code = classify("stvdy://auth/callback?code=%20&state=xyz");
        assert_eq!(
            blank_code,
            DeepLinkIntent::Ignored(Ignored {
                reason: IgnoreReason::MissingCode
            })
        );

        let missing_state = classify("stvdy://auth/callback?code=abc");
        assert_eq!(
            missing_state,
            DeepLinkIntent::Ignored(Ignored {
                reason: IgnoreReason::MissingState
            })
        );

        let blank_state = classify("stvdy://auth/callback?code=abc&state=");
        assert_eq!(
            blank_state,
            DeepLinkIntent::Ignored(Ignored {
                reason: IgnoreReason::MissingState
            })
        );
    }

    /// A `https://` link is now *parsed* rather than rejected outright (A320), so a
    /// foreign host is refused by host rather than by scheme. A scheme that is neither
    /// ours nor `https` is still `NotStvdyScheme`, and both cases are asserted here so
    /// the two reasons cannot collapse into one another.
    #[test]
    fn a_foreign_scheme_is_ignored() {
        assert_eq!(
            classify("https://example.com/auth/callback?code=abc&state=xyz"),
            DeepLinkIntent::Ignored(Ignored {
                reason: IgnoreReason::NotOurHost
            })
        );
        assert_eq!(
            classify("ftp://app.stvdy/auth/callback?code=abc&state=xyz"),
            DeepLinkIntent::Ignored(Ignored {
                reason: IgnoreReason::NotStvdyScheme
            })
        );
    }

    #[test]
    fn an_unknown_route_is_ignored() {
        assert_eq!(
            classify("stvdy://whatever/else"),
            DeepLinkIntent::Ignored(Ignored {
                reason: IgnoreReason::UnknownRoute
            })
        );
    }

    #[test]
    fn malformed_input_never_panics() {
        let inputs = [
            "",
            "stvdy://",
            "stvdy:",
            "stvdy:///",
            "stvdy://auth/callback?",
            "stvdy://auth/callback?code",
            "stvdy://auth/callback?=x",
            "stvdy://%",
            "stvdy://%zz",
            "stvdy://auth/callback?code=%",
            "stvdy://auth/callback?ticket=%",
            "stvdy://auth/callback?ticket",
            "stvdy://auth/callback?ticket=%zz&ticket=b",
            "not a url at all",
            "::://%%%",
            "stvdy://auth/callback?code=%E2%82%AC&state=s",
            "stvdy://auth/callback?code=🙂&state=s",
            "STVDY://AUTH/CALLBACK?code=c&state=s",
        ];
        for input in inputs {
            // Must return, not abort.
            let _ = classify(input);
        }

        // Case-insensitive scheme and route are still recognised.
        assert!(matches!(
            classify("STVDY://AUTH/CALLBACK?code=c&state=s"),
            DeepLinkIntent::AuthCallback(_)
        ));
    }

    #[test]
    fn an_ignored_intent_never_carries_the_url() {
        // A non-`stvdy` link with a credential must not smuggle it into a log
        // line: the ignored payload carries only the reason.
        let intent = classify("https://evil.example/cb?code=SUPERSECRET&state=x");
        let json = serde_json::to_string(&intent).expect("serializes");
        assert!(
            !json.contains("SUPERSECRET"),
            "ignored intent leaked the URL: {json}"
        );
        assert_eq!(
            json,
            r#"{"action":"ignored","payload":{"reason":"not_our_host"}}"#
        );
    }

    #[test]
    fn serializes_to_the_action_payload_shape() {
        let value = serde_json::to_value(classify("stvdy://auth/callback?code=c&state=s"))
            .expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({
                "action": "auth_callback",
                "payload": { "code": "c", "state": "s" }
            })
        );

        let value = serde_json::to_value(classify("stvdy://oauth/calendar?provider=google"))
            .expect("serializes");
        assert_eq!(
            value,
            serde_json::json!({
                "action": "calendar",
                "payload": { "params": { "provider": "google" } }
            })
        );
    }

    #[test]
    fn the_action_label_is_loggable_and_url_free() {
        assert_eq!(
            classify("stvdy://auth/callback?code=c&state=s").action(),
            "auth_callback"
        );
        assert_eq!(classify("stvdy://oauth/calendar?x=1").action(), "calendar");
        assert_eq!(classify("stvdy://nope").action(), "ignored");
    }
}
