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
