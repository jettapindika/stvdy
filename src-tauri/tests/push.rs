//! The push seam: the provider string and the token validation are pure and are
//! asserted here; the plugin's own registration is a device-only path.
//!
//! `unwrap`/`expect`/`panic` are denied in non-test code by this crate's lint
//! block; this is test code, where a failed precondition should stop the test.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

#[test]
fn the_provider_string_matches_what_the_server_accepts() {
    // devices.rs:29 PUSH_PROVIDERS = ["apns", "fcm"]. A third string would be a
    // typo that sits in push_tokens and never delivers.
    assert_eq!(stvdy_shell_lib::push::provider_for("ios"), Some("apns"));
    assert_eq!(stvdy_shell_lib::push::provider_for("android"), Some("fcm"));
    assert_eq!(stvdy_shell_lib::push::provider_for("macos"), None);
}

#[test]
fn an_empty_token_is_refused_before_it_reaches_the_network() {
    assert!(stvdy_shell_lib::push::validate_token("").is_err());
    assert!(stvdy_shell_lib::push::validate_token("abc").is_ok());
}

/// The server rejects a whitespace-only token (`devices.rs`: `token.trim().is_empty()`),
/// so the seam must too — otherwise the PUT is a round trip that can only 422.
#[test]
fn a_whitespace_token_is_refused_and_a_padded_one_is_trimmed() {
    assert!(stvdy_shell_lib::push::validate_token("   ").is_err());
    assert_eq!(
        stvdy_shell_lib::push::validate_token(" abc \n").unwrap(),
        "abc"
    );
}

/// The two provider lists are one contract written twice, so they are compared rather
/// than trusted. `provider_for` returning a string the server's `PUSH_PROVIDERS` does
/// not contain is a token that sits in `push_tokens` and never delivers — a failure
/// with no error anywhere: the PUT succeeds, the row is written, and the phone is
/// silent.
///
/// This reads `crates/server/src/devices.rs` as text, across the workspace boundary.
/// A drift that moves or renames the constant fails this test loudly, which is the
/// intended outcome: the alternative is a check that quietly stops checking.
#[test]
fn the_provider_list_matches_the_servers_own_list() {
    let server = include_str!("../../crates/server/src/devices.rs");
    let declaration = server
        .lines()
        .find(|line| line.trim_start().starts_with("const PUSH_PROVIDERS"))
        .expect("devices.rs still declares PUSH_PROVIDERS");

    for provider in stvdy_shell_lib::push::PROVIDERS {
        assert!(
            declaration.contains(&format!("\"{provider}\"")),
            "the shell reports {provider} but the server does not accept it: {declaration}"
        );
    }
    // And the other direction: a provider the server gained that this shell cannot
    // report is a phone that will never register with it.
    let server_count = declaration.matches('"').count() / 2;
    assert_eq!(
        server_count,
        stvdy_shell_lib::push::PROVIDERS.len(),
        "the server accepts {server_count} providers and the shell knows {}: {declaration}",
        stvdy_shell_lib::push::PROVIDERS.len()
    );
}

/// The route is written twice — here and in `crates/server/src/devices.rs`'s router —
/// and a drift is a `PUT` to a 404 the client would report as a registration failure
/// with no way to tell it from a network one. So the server's own file is read and the
/// two spellings compared, the same way the provider list is.
#[test]
fn the_registration_route_is_the_one_the_server_mounts() {
    let server = include_str!("../../crates/server/src/devices.rs");
    assert!(
        server.contains(".route(\"/devices/{id}/push-token\""),
        "the server still mounts the push-token route; devices.rs:35"
    );
    assert_eq!(
        stvdy_shell_lib::push::token_path("11111111-1111-4111-8111-111111111111"),
        "/devices/11111111-1111-4111-8111-111111111111/push-token"
    );
}

/// A token that could only be refused is refused *before* the request, so a caller
/// cannot turn a validation bug into a round trip and a 422. The URL below is
/// deliberately unroutable: if the check ever moves after `send`, this fails with a
/// network error instead of `EmptyToken`.
#[tokio::test]
async fn a_blank_token_never_reaches_the_network() {
    let client = reqwest::Client::new();
    let token = stvdy_shell_lib::push::PushToken {
        provider: "apns",
        token: "   ".to_string(),
    };
    let error = stvdy_shell_lib::push::register_with_device(
        &client,
        "http://127.0.0.1:1",
        "access-token",
        "11111111-1111-4111-8111-111111111111",
        &token,
    )
    .await
    .expect_err("a whitespace token is refused");
    assert!(matches!(
        error,
        stvdy_shell_lib::push::PushError::EmptyToken
    ));
}

/// A registration that the server refuses carries the status and the server's own
/// body: "the PUT failed" with no code is a bug report nobody can act on.
#[tokio::test]
async fn a_refused_registration_reports_the_status_and_the_body() {
    use std::io::{Read, Write};

    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a scratch port");
    let address = listener.local_addr().expect("an address");
    std::thread::spawn(move || {
        if let Ok((mut stream, _)) = listener.accept() {
            let mut buffer = [0u8; 2048];
            let _ = stream.read(&mut buffer);
            let body = r#"{"error":"provider must be \"apns\" or \"fcm\""}"#;
            let response = format!(
                "HTTP/1.1 422 Unprocessable Entity\r\ncontent-type: application/json\r\ncontent-length: {}\r\nconnection: close\r\n\r\n{body}",
                body.len()
            );
            let _ = stream.write_all(response.as_bytes());
        }
    });

    let client = reqwest::Client::new();
    let token = stvdy_shell_lib::push::PushToken {
        provider: "apns",
        token: "a-real-token".to_string(),
    };
    let error = stvdy_shell_lib::push::register_with_device(
        &client,
        &format!("http://{address}"),
        "access-token",
        "11111111-1111-4111-8111-111111111111",
        &token,
    )
    .await
    .expect_err("a 422 is a refusal");
    match error {
        stvdy_shell_lib::push::PushError::Refused { status, body } => {
            assert_eq!(status, 422);
            assert!(
                body.contains("provider"),
                "the server's own message: {body}"
            );
        }
        other => panic!("expected a refusal, got {other}"),
    }
}
