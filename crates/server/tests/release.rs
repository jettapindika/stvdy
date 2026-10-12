//! The deployment's two modes, and the guarantee each one makes.
//!
//! `--migrate-only` exists because of a real race: `main.rs` migrates on every boot, so two
//! replicas starting together both run `sqlx::migrate!` against one database. The advisory
//! lock makes that survivable, but the replica that loses the race then serves traffic against
//! a schema it has not verified — so the deploy procedure runs the migration to completion
//! **before** any replica starts (A346).
//!
//! That is why the assertion here is not "it exited 0": a process that bound a port and then
//! crashed also exits non-zero, and a process that bound a port and *kept serving* would be
//! exactly the thing the flag is supposed to prevent. The test drives the real binary and
//! asserts both halves — the schema moved, and nothing is listening.
//!
//! A process-level test is the honest shape for a flag: an in-process call to a `migrate_only`
//! function would prove the function works and say nothing about whether `main` routes the
//! argument to it.

// `#[sqlx::test]` is a macro harness, so clippy.toml's test relaxations are not guaranteed to
// apply to the generated code.
#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::process::Command;

use sqlx::{Executor, PgPool};

/// The repository root: the binary's relative config paths resolve against its working
/// directory, so the test runs it from here.
fn root() -> std::path::PathBuf {
    std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("../..")
}

fn binary() -> &'static str {
    env!("CARGO_BIN_EXE_stvdy-server")
}

/// A `postgres://` URL for a database on the same server as `pool`.
///
/// Built from the pool's own connect options rather than from `DATABASE_URL`, because
/// `#[sqlx::test]` gives each case its own scratch database and the environment variable names
/// the server, not that database.
fn url_for(pool: &PgPool, database: &str) -> String {
    let options = pool.connect_options();
    format!(
        "postgres://{}@{}:{}/{}",
        options.get_username(),
        options.get_host(),
        options.get_port(),
        database
    )
}

/// A port that is free right now, so the probe below has something to fail to connect to.
fn free_port() -> u16 {
    let listener = std::net::TcpListener::bind("127.0.0.1:0").expect("a loopback port");
    listener.local_addr().expect("the bound address").port()
}

/// `--migrate-only` migrates a fresh database and exits without binding a port.
///
/// The database is created by this test and **not** migrated by the harness, so "the schema
/// moved" is a statement about the binary rather than about `#[sqlx::test]`.
#[sqlx::test(migrations = "../../migrations")]
async fn migrate_only_exits_zero_without_binding(pool: PgPool) {
    let database = format!("stvdy_migrate_only_{}", uuid::Uuid::new_v4().simple());
    // `AssertSqlSafe` is sqlx 0.9's opt-in for a dynamic statement, and the `String` form is
    // the one that owns it: the `&str` form borrows, which cannot outlive this `await`.
    // The statement is not injectable — the only interpolated value is a UUID this test made.
    pool.execute(sqlx::AssertSqlSafe(format!("create database {database}")))
        .await
        .expect("a scratch database");

    let url = url_for(&pool, &database);
    let port = free_port();

    let output = Command::new(binary())
        .arg("--migrate-only")
        .current_dir(root())
        .env("DATABASE_URL", &url)
        .env("BIND_ADDR", format!("127.0.0.1:{port}"))
        .env("APP_ENV", "dev")
        .output()
        .expect("the binary runs");

    let stdout = String::from_utf8_lossy(&output.stdout);
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        output.status.success(),
        "--migrate-only must exit 0; got {:?}\nstdout: {stdout}\nstderr: {stderr}",
        output.status
    );

    // The schema moved, so the exit code is not the only evidence that it did anything: a
    // process that parsed the flag and returned would pass an exit-code check.
    //
    // The migration list is what the *image's* step has to preserve, and the invariant this
    // phase must not disturb — that `0017_sync_triggers.sql` stays the last trigger-attaching
    // migration — is asserted once, in `crates/server/tests/schema.rs`. Deliberately not
    // repeated here: a second check of the same fact is a second thing to keep in step, and
    // the first one already fails.
    let migrated = sqlx::PgPool::connect(&url)
        .await
        .expect("the migrated database is reachable");
    let applied: i64 = sqlx::query_scalar("select count(*) from _sqlx_migrations")
        .fetch_one(&migrated)
        .await
        .expect("the migrations table exists, so migrations ran");
    assert!(applied > 0, "no migration was applied");

    // The plans are seeded by the same step, and a replica that started without them would
    // fail its own boot assertion rather than serve.
    let plans: i64 = sqlx::query_scalar("select count(*) from plans")
        .fetch_one(&migrated)
        .await
        .expect("the plans table exists");
    assert!(plans > 0, "--migrate-only must seed the plans too");

    // **The assertion that matters.** "It exited 0" is also true of a process that bound a
    // port and then crashed, and of one that bound a port and kept serving — which is the
    // state the flag exists to make impossible.
    assert!(
        !stdout.contains("listening"),
        "--migrate-only must not bind a port; stdout: {stdout}"
    );
    let bound = std::net::TcpStream::connect(("127.0.0.1", port));
    assert!(
        bound.is_err(),
        "--migrate-only left something listening on {port}"
    );

    migrated.close().await;
    pool.execute(sqlx::AssertSqlSafe(format!("drop database {database}")))
        .await
        .expect("the scratch database drops");
}

/// A flag that is not one of the two is refused rather than ignored.
///
/// The failure this catches is a deployment that misspells `--migrate-only`, starts the server
/// instead, and runs the migration from every replica — which is precisely the race the flag
/// was added to remove, reached by a typo.
///
/// No database on purpose, and that is the ordering assertion: the argument is parsed *before*
/// the pool is opened, so a typo is discovered before a migration has run rather than after.
/// A test that supplied `DATABASE_URL` could not tell the two orders apart.
#[test]
fn an_unknown_argument_is_refused() {
    let output = Command::new(binary())
        .arg("--migrate-onyl")
        .current_dir(root())
        .env_remove("DATABASE_URL")
        .env("APP_ENV", "dev")
        .output()
        .expect("the binary runs");

    assert!(
        !output.status.success(),
        "a misspelled flag must not start the server"
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown argument"),
        "the refusal must name the argument, and it must happen before the pool opens: {stderr}"
    );
}

// ---------------------------------------------------------------------------
// The update channel (Phase 15, Task 1).
//
// The updater is the one place in this product where a forged artifact is a real
// threat rather than a hypothetical: the app trusts a downloaded binary because a
// minisign signature over the manifest verifies against a public key compiled into
// it. So what these cases exercise is the *procedure* — on every commit, with a
// fixture keypair — rather than the operator's real key on release day (A332).
//
// The production private key is never in this repository and never in this test:
// it is a GitHub Actions secret, and `scripts/verify-release-artifacts.sh` refuses
// to run if it is set in the environment. `minisign` (which can sign) is therefore
// a dev-dependency of this crate, never a dependency of it.
//
// None of these cases needs a database, which is why this is a plain suite.
// ---------------------------------------------------------------------------

/// The repository root, reusing the helper above: these cases read committed files.
fn repo_file(relative: &str) -> String {
    let path = root().join(relative);
    std::fs::read_to_string(&path).unwrap_or_else(|error| panic!("{}: {error}", path.display()))
}

fn json_file(relative: &str) -> serde_json::Value {
    serde_json::from_str(&repo_file(relative))
        .unwrap_or_else(|error| panic!("{relative} is not JSON: {error}"))
}

/// One capability file's `permissions`, as strings.
fn capability_permissions(name: &str) -> Vec<String> {
    json_file(&format!("src-tauri/capabilities/{name}"))["permissions"]
        .as_array()
        .unwrap_or_else(|| panic!("{name} has no `permissions` array"))
        .iter()
        .map(|entry| {
            entry
                .as_str()
                .expect("a permission is a string")
                .to_string()
        })
        .collect()
}

/// The `base64(<file contents>)` wrapper the plugin puts around both the public key
/// and the manifest signature.
fn plugin_base64(plain: &str) -> String {
    use base64::Engine as _;
    base64::engine::general_purpose::STANDARD.encode(plain.as_bytes())
}

/// Verifies exactly the way `tauri-plugin-updater` 2.13.3 does.
///
/// The plugin's path is `base64 → utf8 → minisign_verify::{PublicKey, Signature}`
/// `::decode → verify(bytes, signature, allow_legacy = true)`. Reproducing it here
/// rather than calling a helper of our own is the point: a test that verified with
/// a different function would prove something about that function.
fn verify_signed_manifest(
    manifest: &[u8],
    signature_b64: &str,
    pubkey_b64: &str,
) -> Result<(), String> {
    use base64::Engine as _;
    let decode = |value: &str| -> Result<String, String> {
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(value)
            .map_err(|error| error.to_string())?;
        String::from_utf8(bytes).map_err(|error| error.to_string())
    };

    let public_key =
        minisign_verify::PublicKey::decode(&decode(pubkey_b64)?).map_err(|e| format!("{e:?}"))?;
    let signature = minisign_verify::Signature::decode(&decode(signature_b64)?)
        .map_err(|e| format!("{e:?}"))?;

    public_key
        .verify(manifest, &signature, true)
        .map_err(|error| format!("{error:?}"))
}

/// A signed manifest verifies, a mutated one does not, and an unsigned one does not.
///
/// This is the verification half of the update channel, exercised on every commit
/// instead of on release day. The fixture keypair is generated here and is NOT the
/// production key: what is asserted is the *procedure* — minisign over the manifest
/// bytes, against the public key the config commits. The production key's presence
/// is asserted separately by `the_bundle_creates_updater_artifacts_and_names_a_public_key`.
///
/// Every case below is one an attacker who can serve a response but not sign one is
/// trying, in the order they would try it: serve a different manifest, serve the
/// real manifest with a mangled byte, serve no signature at all, and sign with a
/// key of their own.
#[test]
fn a_signed_manifest_verifies_and_a_mutated_one_does_not() {
    let keypair = minisign::KeyPair::generate_unencrypted_keypair().expect("a fixture keypair");
    // Shaped like the plugin's dynamic response (`url` + `signature`), because that
    // is the response whose bytes the signature covers.
    let manifest = br#"{"version":"0.2.0","notes":"Fixes the PDF export.","pub_date":"2026-10-12T00:00:00Z","url":"https://updates.stvdy.app/Stvdy_0.2.0_aarch64.app.tar.gz","signature":"ZXhhbXBsZQ=="}"#;

    let signature = minisign::sign(
        Some(&keypair.pk),
        &keypair.sk,
        std::io::Cursor::new(manifest),
        // The Tauri CLI writes the version into the trusted comment and the plugin
        // can be configured to require it (`require_signed_version`), so the fixture
        // is signed the way a real release is. The comment is covered by the global
        // signature, which is why the plugin reads it only after `verify`.
        Some("version:0.2.0"),
        Some("untrusted comment: signature from the Phase 15 fixture key"),
    )
    .expect("the fixture key signs the manifest");

    let pubkey_b64 = plugin_base64(&keypair.pk.to_box().expect("a public key box").into_string());
    let signature_b64 = plugin_base64(&String::from(signature));

    // (1) Valid → Ok. Without this the two negatives below would pass against a
    // verifier that rejected everything.
    verify_signed_manifest(manifest, &signature_b64, &pubkey_b64)
        .expect("a manifest signed by the committed key must verify");

    // (2) One flipped byte → Err. The mutation is inside the version string, so a
    // verifier that checked only the signature's shape or the trusted comment would
    // still accept it.
    let mut mutated = manifest.to_vec();
    mutated[12] ^= 0x01;
    assert_ne!(
        mutated, manifest,
        "the mutation must actually change the bytes"
    );
    assert!(
        verify_signed_manifest(&mutated, &signature_b64, &pubkey_b64).is_err(),
        "a one-byte mutation of a signed manifest must not verify"
    );

    // (3) No signature → Err. An unsigned manifest is the "no signature at all"
    // case: both the empty string and a base64-wrapped empty string must be refused
    // rather than treated as "nothing to check".
    //
    // **Honest about *why* these fail.** Both are rejected by the signature's own
    // decoder, before any cryptography runs — a manifest whose `signature` field is
    // absent or empty is a manifest the plugin's deserializer refuses outright
    // (`the `signature` field was not set on the updater response`). So these two
    // assert the *structural* refusal, and the case below is what asserts the
    // cryptographic one.
    assert!(
        verify_signed_manifest(manifest, "", &pubkey_b64).is_err(),
        "a manifest with no signature must not verify"
    );
    assert!(
        verify_signed_manifest(manifest, &plugin_base64(""), &pubkey_b64).is_err(),
        "a manifest with an empty signature must not verify"
    );

    // (3b) A well-formed signature over *different* bytes → Err.
    //
    // This is the case the empty ones cannot see, and it is the replay an attacker
    // would actually attempt: take a genuine `.sig` from an older release, pair it
    // with a manifest you wrote, and let the app read the version it names. The
    // signature decodes, the key id matches, and only the Ed25519 verification
    // rejects it — so unlike (3) this one is a statement about the cryptography.
    let other = minisign::sign(
        Some(&keypair.pk),
        &keypair.sk,
        std::io::Cursor::new(b"{\"version\":\"9.9.9\"}"),
        Some("version:9.9.9"),
        Some("untrusted comment: signature from the Phase 15 fixture key"),
    )
    .expect("the fixture key signs a different manifest");
    assert!(
        verify_signed_manifest(manifest, &plugin_base64(&String::from(other)), &pubkey_b64)
            .is_err(),
        "a signature over different bytes must not verify"
    );

    // (4) A signature from someone else's key → Err. This is the forged-manifest
    // case the phase exists to prevent, and the cases above cannot see it: the bytes
    // are genuine and the signature is well-formed and correctly made, it just is
    // not ours.
    //
    // The second assertion is what makes the first one mean "the key is pinned"
    // rather than "signatures do not work": the very same signature verifies against
    // the key that produced it, so pinning the committed public key is the only
    // thing rejecting it.
    let attacker = minisign::KeyPair::generate_unencrypted_keypair().expect("a second keypair");
    let forged = minisign::sign(
        None,
        &attacker.sk,
        std::io::Cursor::new(manifest),
        Some("version:0.2.0"),
        Some("untrusted comment: signature from a key that is not ours"),
    )
    .expect("the attacker's key signs the same manifest");
    let forged_b64 = plugin_base64(&String::from(forged));
    assert!(
        verify_signed_manifest(manifest, &forged_b64, &pubkey_b64).is_err(),
        "a manifest signed by a different key must not verify"
    );
    let attacker_pubkey_b64 = plugin_base64(
        &attacker
            .pk
            .to_box()
            .expect("a public key box")
            .into_string(),
    );
    assert!(
        verify_signed_manifest(manifest, &forged_b64, &attacker_pubkey_b64).is_ok(),
        "the forged signature must be valid against its own key, or the case above proves nothing"
    );
}

/// The bundle produces updater artifacts and names the key that verifies them.
///
/// `createUpdaterArtifacts` is what makes `tauri build` emit the `.tar.gz`/`.sig`
/// pair the manifest points at, and `pubkey` is the half of the trust chain that
/// lives in this repository (A332). Both are one-line facts that a release would
/// otherwise discover by shipping an artifact nobody can verify.
#[test]
fn the_bundle_creates_updater_artifacts_and_names_a_public_key() {
    let config = json_file("src-tauri/tauri.conf.json");

    assert_eq!(
        config["bundle"]["createUpdaterArtifacts"],
        serde_json::json!(true),
        "the bundle must create updater artifacts, or the release has nothing to publish"
    );

    let updater = &config["plugins"]["updater"];
    let endpoints = updater["endpoints"]
        .as_array()
        .expect("`plugins.updater.endpoints` is an array; the plugin requires it");
    assert_eq!(
        endpoints.len(),
        1,
        "one endpoint, so a release cannot be served from two places that disagree: {endpoints:?}"
    );
    let endpoint = endpoints[0].as_str().expect("an endpoint is a string");
    // The plugin refuses a non-HTTPS endpoint unless
    // `dangerous_insecure_transport_protocol` is set, and that flag must never be
    // set: an update fetched over plain HTTP is an update an attacker chooses.
    assert!(
        endpoint.starts_with("https://"),
        "the update endpoint must be HTTPS: {endpoint}"
    );
    assert!(
        updater["dangerous_insecure_transport_protocol"].is_null(),
        "the insecure-transport escape hatch must not be committed"
    );
    for placeholder in ["{{target}}", "{{arch}}", "{{current_version}}"] {
        assert!(
            endpoint.contains(placeholder),
            "the endpoint must carry {placeholder}, or every platform resolves to one artifact"
        );
    }

    // Presence, not validity. The value is the operator's key (A332): until they
    // paste it, the committed placeholder is a string the plugin deserializes and
    // then refuses every signature against, which is a safe-but-useless updater
    // rather than a startup panic. What proves the *real* key is in place is the
    // release gate — `scripts/verify-release-artifacts.sh --manifest` verifies the
    // published `latest.json` against this committed value, so a placeholder fails
    // the release rather than shipping.
    let pubkey = updater["pubkey"]
        .as_str()
        .expect("`plugins.updater.pubkey` is a string; the plugin requires it");
    assert!(
        !pubkey.trim().is_empty(),
        "the committed public key must not be blank"
    );

    // And the Windows install mode the updater itself uses, which is not the
    // installer's: `passive` shows progress without asking, `basicUi` shows the
    // wizard, and a silent install of an update the user did not accept is the
    // thing A336 forbids.
    assert_eq!(
        updater["windows"]["installMode"],
        serde_json::json!("passive"),
        "the updater's own Windows install mode must be passive"
    );
}

/// The updater permission is desktop-only, and the mobile builds cannot see it.
///
/// A plugin registered but not permitted is a runtime failure in the bundled app
/// and invisible to `cargo test`, which is the whole reason this is asserted here.
/// The other half matters just as much: `tauri-plugin-updater` declares
/// `ios: none, android: none`, so a permission granted to a mobile build is either
/// dead or a capability the store build would fail on (A335).
#[test]
fn the_updater_is_desktop_only() {
    let desktop = json_file("src-tauri/capabilities/desktop.json");

    assert!(
        capability_permissions("desktop.json").contains(&"updater:default".to_string()),
        "`updater:default` must be in desktop.json, or the plugin is registered and unusable"
    );
    // The capability's own platform list is the plugin's platform matrix, so the
    // permission cannot reach a mobile build even by a `default.json` mistake.
    let platforms: Vec<&str> = desktop["platforms"]
        .as_array()
        .expect("desktop.json lists its platforms")
        .iter()
        .map(|entry| entry.as_str().expect("a platform is a string"))
        .collect();
    assert_eq!(platforms, vec!["macOS", "windows", "linux"]);

    for file in ["default.json", "mobile.json"] {
        assert!(
            !capability_permissions(file).contains(&"updater:default".to_string()),
            "`updater:default` must not be in {file}: the permission is desktop-only"
        );
    }
}

/// The macOS bundle pins the hardened runtime, the floor and the entitlements.
///
/// `hardenedRuntime` defaults to `true` in the schema, so leaving it out is not a
/// bug today — it is a bug the day the default changes, and a notarized build is
/// exactly the thing that would then fail in a way nobody reads. The floor is
/// pinned for the same reason: it is a support decision, not a build detail.
#[test]
fn the_macos_bundle_pins_hardened_runtime_and_the_minimum_version() {
    let macos = &json_file("src-tauri/tauri.conf.json")["bundle"]["macOS"];

    assert_eq!(
        macos["hardenedRuntime"],
        serde_json::json!(true),
        "the hardened runtime must be pinned, not inherited from the schema default"
    );
    assert_eq!(
        macos["minimumSystemVersion"],
        serde_json::json!("11.0"),
        "the support floor is a decision; changing it is a deliberate edit"
    );
    assert_eq!(
        macos["entitlements"],
        serde_json::json!("entitlements.plist"),
        "the shell signs with its own entitlements, not Tauri's defaults"
    );
    // Deliberately absent: the workflow passes `APPLE_SIGNING_IDENTITY` from a
    // secret, so a contributor's build is unsigned rather than failing on an
    // identity they do not have. Committing one here would break every other
    // machine's build.
    assert!(
        macos.get("signingIdentity").is_none(),
        "`signingIdentity` must not be committed; it comes from the workflow's secret"
    );

    // **The config names a file, so the file has to be there and has to be the
    // right file.** `bundle.macOS.entitlements` is a path, and a path that does not
    // resolve is a `tauri build` failure on macOS — one that only a macOS runner
    // sees, which is the same class of defect as a plugin registered without its
    // permission. This asserts the path resolves and that the file says what the
    // plan says it says.
    let entitlements_path = root().join("src-tauri").join("entitlements.plist");
    let entitlements = std::fs::read_to_string(&entitlements_path).unwrap_or_else(|error| {
        panic!(
            "`bundle.macOS.entitlements` names a file that must exist: {}: {error}",
            entitlements_path.display()
        )
    });

    // Exactly the two the shell needs (A333). The second assertion is the one that
    // matters: an entitlement added "just in case" is a hole that survives review,
    // so an extra `com.apple.security.*` key is a failure rather than a curiosity.
    let expected = [
        "com.apple.security.cs.allow-jit",
        "com.apple.security.cs.allow-unsigned-executable-memory",
    ];
    for key in expected {
        assert!(
            entitlements.contains(key),
            "the entitlements file must carry {key}"
        );
    }

    // Comments are stripped before the keys are counted: this file's own comment
    // names `disable-library-validation` as the key it is deliberately *not*
    // granting, and a mention in a comment is not a grant.
    let mut body = String::new();
    let mut in_comment = false;
    for line in entitlements.lines() {
        let line = line.trim();
        if in_comment {
            in_comment = !line.contains("-->");
            continue;
        }
        if line.starts_with("<!--") {
            in_comment = !line.contains("-->");
            continue;
        }
        body.push_str(line);
        body.push('\n');
    }

    let mut granted: Vec<&str> = body
        .match_indices("com.apple.security.")
        .map(|(at, _)| {
            let rest = &body[at..];
            let end = rest
                .find(|c: char| !(c.is_ascii_alphanumeric() || c == '.' || c == '-'))
                .unwrap_or(rest.len());
            &rest[..end]
        })
        .filter(|key| !expected.contains(key))
        .collect();
    granted.sort_unstable();
    granted.dedup();
    assert!(
        granted.is_empty(),
        "the hardened runtime blocks what is not granted; these are not needed: {granted:?}"
    );
}

/// The Windows installer is per-machine, SHA-256 and timestamped.
///
/// A timestamp is what keeps a signed installer verifiable after the certificate
/// expires, and SHA-256 is what `osslsigncode verify` in the release gate checks.
/// Without either, the installer still builds — it just stops verifying later,
/// which is a defect nobody sees until they need it.
#[test]
fn the_windows_installer_is_per_machine_and_sha256_timestamped() {
    let windows = &json_file("src-tauri/tauri.conf.json")["bundle"]["windows"];

    assert_eq!(
        windows["nsis"]["installMode"],
        serde_json::json!("perMachine"),
        "the installer installs for every user, as A334 pins"
    );
    assert_eq!(
        windows["digestAlgorithm"],
        serde_json::json!("sha256"),
        "SHA-256, or the signature is over a digest nobody accepts"
    );
    let timestamp = windows["timestampUrl"]
        .as_str()
        .expect("a timestamp URL is required for a signature that outlives the certificate");
    assert!(
        timestamp.starts_with("http"),
        "the timestamp URL must be a URL: {timestamp}"
    );
}

/// The shell's version is the workspace's, so a release cannot ship `0.1.0` twice.
///
/// Two files that must agree, and a test that says so, beats a release that
/// discovers the drift in the manifest URL (A353). The updater compares the
/// *running* version against the version the manifest announces, so a build whose
/// config and crate disagree is a build that offers itself the update it already
/// has — or never offers one at all.
#[test]
fn the_version_is_the_workspace_version() {
    let configured = json_file("src-tauri/tauri.conf.json")["version"]
        .as_str()
        .expect("the config carries a version")
        .to_string();

    let workspace: toml::Value =
        toml::from_str(&repo_file("Cargo.toml")).expect("the workspace manifest parses");
    let workspace_version = workspace["workspace"]["package"]["version"]
        .as_str()
        .expect("the workspace pins a version")
        .to_string();
    assert_eq!(
        configured, workspace_version,
        "`tauri.conf.json`'s version and `[workspace.package] version` must agree"
    );

    // A353 is the other half of the same fact: `tauri.conf.json`'s version is the
    // *shell's*, and the server reports its own from `CARGO_PKG_VERSION`. They
    // agree today; when they stop, this message says which contract moved.
    let shell: toml::Value =
        toml::from_str(&repo_file("src-tauri/Cargo.toml")).expect("the shell's manifest parses");
    let shell_version = shell["package"]["version"]
        .as_str()
        .expect("the shell pins a version");
    assert_eq!(
        configured, shell_version,
        "A353: `tauri.conf.json`'s version is the shell's own, so the two move together"
    );
}
