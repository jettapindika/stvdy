# Handover — what the user needs to do

Written while the remaining phases execute. Everything that could be done without you has
been; this is the list of things that cannot be.

## Launch blockers found in the code, not in `ASSUMPTIONS.md`

These are **code gaps**, not credential gaps, and each one stops a store submission on its own.
They were found while writing the store checklists (Phase 15), and they are recorded here because
nothing in a phase's own file list owned them:

| # | What is missing | Why it blocks |
|---|---|---|
| **In-app account deletion** | **Shipped (A355).** `DELETE /account` and `GET /account/export` in `crates/server/src/account.rs`, the `purge_deleted_accounts` handler in `crates/server/src/jobs/handlers/purge_accounts.rs`, and the settings screen's data section with a type-to-confirm step | No longer a blocker. Apple requires in-app deletion of an account an app can create, and that is now `code` in `docs/RELEASE.md` (A2) |
| **The Play account-deletion URL** | The public **page** does not exist. The route behind it does: `DELETE /account` ships with the app | Google Play requires a public deletion URL for any app with account creation. The page is a deployment artefact rather than code, and it needs the production host — `docs/RELEASE.md` G2 is typed `user` for that reason |
| **The iOS build number** | ~~No build version in the config~~ — **fixed**: `bundle.iOS.bundleVersion` and `bundle.macOS.bundleVersion` are now `"1"`. Note the key is **per-platform**: `bundle.bundleVersion` is not in the CLI schema's `BundleConfig` at all (`app/node_modules/@tauri-apps/cli/config.schema.json`, `IosConfig`/`MacConfig` each define `bundleVersion` as "translates to CFBundleVersion"), so the top-level form would be rejected or silently ignored. Bumping the build number is now a one-line diff rather than a shell-version bump | A second submission without a higher build number is rejected as a duplicate |

The first two are the same feature, and **it is implemented and committed** — `crates/server/src/account.rs`, `crates/server/src/jobs/handlers/purge_accounts.rs`, `migrations/0037_account_purge.sql`, the two routes, the Settings Data section, and the retention/checklist documents corrected (`c049b43`, A355). What remains for it is not code: the **public Play deletion URL** needs the production host, and the Apple/Google review notes need your accounts.

The gap was a product-scope decision rather than a task: it needed the retention/purge machinery, a confirm step, and copy that says what is deleted. **A160 is the reason it went unowned** — Phase 7's plan re-tagged the `## Account` heading away from itself on the grounds that the work "belongs with the release prerequisites", and no phase's file list then picked it up, so it sat as a promise in `docs/API.md`, `docs/OPERATIONS.md`'s retention table and `docs/EULA.md` § 9 while nothing implemented it.

## One environment note worth acting on

**The machine ran out of disk twice, and the second time the cause was found.** `/System/Volumes/Data` hit **105 MB free** early in the session and **117 MB** again on 2026-10-12, and both times a `cargo test` link died with `ld: write() failed, errno=28`. A failed link is the disk, not the source — the signature is the library compiling clean while the test binary fails to link. The other casualties were Phase 13's test run, Phase 15's one local `docker build`, and the Docker daemon itself (`docker info` fails; `~/Library/Containers/com.docker.docker` is 6.2 GB).

**The cause, and it is not the size of the workspace.** `target/debug/deps` was **98 GB**, and most of it was **duplicate artifacts of the same crate built under different flag sets** — `cargo test --workspace` and `cargo test -p <crate>` produce different hashes, and each keeps a full rlib plus a linked test binary. `libstvdy_server-*.rlib` alone was **27 files, 3.3 GB**, and there were **420 linked test binaries over 10 MB, 31 GB**. `cargo clean -p stvdy-server -p stvdy-core -p stvdy-sync` finished in 105 seconds and left the third-party dependency cache intact, so the rebuild is the three workspace crates rather than the world. **How much it freed: report the observable delta, not cargo's own sum, because the two disagree by 1.7× and the tool's number is the one that cannot be checked.** Cargo printed `Removed 645667 files, 151.0GiB total`; the two measurements taken either side of it say **`target/debug/deps` 98 GB → 13 GB (85 GB)** and **`df` used 414 GiB → 326 GiB (88 GB)**. The gap is almost certainly hardlinks — `cargo` counts every path it unlinks, and the incremental caches it also cleared are hardlinked trees, so the same blocks are counted more than once. **Use 85–88 GB**; the 151 GB figure is cargo's accounting, not disk that came back.

**What to run when it fills again** — and this is the cheap one, unlike a full `cargo clean`:

```bash
cd /Users/mac/Documents/Coding/project/Stvdy
cargo clean -p stvdy-server -p stvdy-core -p stvdy-sync
rm -rf target/debug/incremental src-tauri/target/debug/incremental
```

**The incremental directories regrow and are always safe to delete** (rustc regenerates them; the cost is rebuild time only) — 3.9 GB and 278 MB on the second pass, and they had been cleared twice before that.

**Note the two target directories — a `cargo clean` in the repo root does not touch the shell's.** `src-tauri` is its own workspace root (deliberately: `tauri_build::build()` must not run during `cargo clippy --workspace`), so its target survives a root `cargo clean` and needs its own:

```bash
cd /Users/mac/Documents/Coding/project/Stvdy/src-tauri && cargo clean -p stvdy-shell
```

**The order that costs only time:** stop every slice, then the clean, then one `cargo test --workspace`. A `cargo clean` while slices are building fails every live invocation on the `target/` lock, which reads as a code failure rather than a disk one. Two more things outside the repo are yours to judge: `~/Library/Caches/com.spotify.client` (3.1 GB) and `~/.cache/huggingface` (5.7 GB, not this project's — the embedder uses `FASTEMBED_CACHE_DIR`).

**And a coupling that cost a gate run here, worth knowing before it costs another.** `just check-shell` runs the shell's doctests, and `tauri::generate_context!()` is a **proc macro that panics at compile time** when `frontendDist` (`../app/dist`) is absent. The justfile already documents the dependency (`check-shell: check-web`, with the reason written out), so a fresh checkout is fine — but if something deletes `app/dist` *between* the two recipes, the gate dies with `error: proc macro panicked … The frontendDist configuration is set to "../app/dist" but this path doesn't exist`, which reads like a broken build script rather than a missing directory. It happened here while a second slice was working in the same tree, and it is not a code defect: `npm --prefix app run build` and a re-run of `just check-shell` → **exit 0**. The general shape is the one this file keeps recording — **two agents in one working tree will trip each other's preconditions**, and the artifact directory is shared state.

## Standing blockers (from `docs/ASSUMPTIONS.md` § Blockers)

These need **you**, not code, and they block a public launch rather than a phase:

| # | What is needed | Why it cannot be automated |
|---|---|---|
| B1 | QRIS gateway identity + API docs, and a PJP to settle through | **Resolved 2026-10-13:** the gateway is the self-hosted `gopay-bridge` over a GoPay merchant account, and the server speaks its API (`crates/server/src/integrations/payment/mapping.rs`). What still needs you is a **phone**: the bridge logs in to the merchant account by SMS OTP and that session expires, so it must be renewed by hand (`docs/OPERATIONS.md` § Payments has the symptom and the two commands). And no real payment has been taken through it yet — launch checklist row 11 stays blocked until one settles |
| B2 | Tax/PPN treatment and store privacy-label text | Legal, not technical. Invoices must be correct and the UU PDP privacy notice must be true |
| B3 | SMTP host/user/password and a From address | Every AI action is gated on a verified email, so with the dev file sink no real account can be onboarded. The provider swap is config-only once the credentials exist |

## GitHub Actions is billing-blocked, and it stops every workflow

**Every job dispatched after 2026-10-12 18:10 WIB was refused before it started**, with the
annotation GitHub puts on the check run:

```
The job was not started because recent account payments have failed or your
spending limit needs to be increased. Please check the 'Billing & plans' section
in your settings
```

This is an account setting, not a defect in any workflow, and **it is yours to clear** —
`Settings → Billing and plans`. The symptom is worth recognising because it looks like a code
failure from the outside: the run goes `completed/failure` in seconds, every job after the
first is `skipped`, and `gh run view --log-failed` answers `log not found` because no runner
ever produced a log. `gh api repos/<owner>/<repo>/check-runs/<job-id>/annotations` is where
the real message is.

What it cost here: `.github/workflows/unsigned-artifacts.yml`'s first green run — the Windows
NSIS installer — had already landed, and the two jobs after it (macOS, Android) could not be
re-run once the block began. **The last run that actually executed is
`38239252653`** (`a28ed56f`), and its artifacts are still downloadable from the Actions tab.

Two consequences worth acting on when the block clears:

- The macOS and Android legs of `unsigned-artifacts.yml` have fixes committed that **have
  never run on a runner** — see the two commits after `a28ed56f`. Both were reproduced and
  fixed locally instead (the dmg builds and is universal; the Gradle block compiles), so what
  is unverified is the runner, not the code.
- `release.yml` is the signed path and it needs its certificates regardless, so it is blocked
  on credentials as well as on billing.

## The installers carry a build-time API origin, so they have to be built for a server

**The client's API address is inlined at build time, not configurable at run time.**
`app/src/api/client.ts` reads `import.meta.env.VITE_API_BASE_URL`, Vite substitutes it as a
literal, and the fallback when it is unset is `http://localhost:8080`. So an installer is
bound to one server when it is built, and **the exe that `unsigned-artifacts.yml` produced
before `a1c3caa` points at `localhost:8080`** — that workflow set no `VITE_API_BASE_URL` at
all (verified against the workflow at `a28ed56`), so that artifact cannot reach any deployed
server. The dmg and the APK attached to v0.1.2 were rebuilt locally against the staging host.

`unsigned-artifacts.yml` now takes `api_base_url` as a **required** input, exports it
job-wide, and refuses the development fallback before building. Its default is
`https://stvdy.ztvn.my.id` — the tunnel's origin, which is the only address that survives the
trial host being replaced (it was replaced on 2026-10-13; see A45). A re-run with
`api_base_url=https://stvdy.ztvn.my.id` produces three correct artifacts in one pass.

**What the CORS allowlist does and does not require.** The server always allows the three
Tauri webview origins (`tauri://localhost`, `https://tauri.localhost`,
`http://tauri.localhost` — `crates/server/src/routes/mod.rs`), so a **desktop or mobile**
installer needs nothing added to `WEB_ORIGIN`; that variable is for a browser serving the SPA.
The API origin still has to be reachable. **That is now the domain rather than the raw IP**: the
previous instance was published on `:8080` behind an Oracle VCN ingress rule for that port, and
the current one publishes nothing — nginx serves the SPA and `/api/` on `:80` and the tunnel
carries it. So an artifact built against the domain is the one that keeps working, and it is
**TLS** rather than cleartext, which retires the operator-only caveat this paragraph used to
carry: a dmg or APK pointing at `https://stvdy.ztvn.my.id` is still a staging build against a
trial host, but the hop is no longer in the clear. Row 7 is about the *production* host's
certificate and is unaffected.

## Staging on the Batam trial host, and it is live

The A45 host (`129.225.10.139`) runs the real stack — Postgres + pgvector, the server image,
the SPA, with SearXNG beside it on the same compose project — **and it is reachable from outside**:
both firewall layers are open and the SPA signs a student up, verifies the address and logs in
through a real browser. `docs/OPERATIONS.md` § Staging is the procedure. **Eleven defects were found
by building and running it there, and every one of them is
fixed or recorded for a decision**; none was visible from the repository, which is the whole reason
the image is worth building and the stack worth running on a host with a Docker daemon:

1. **`cargo build` could not link `ort-sys`'s prebuilt ONNX Runtime** on `rust:1.94.1-bookworm`
   (glibc 2.36 against the `GLIBC_2.38` symbols `__isoc23_strtoll`, `__isoc23_strtoull`,
   `_M_replace_cold`). The base image moved to trixie and `CHROMIUM_VERSION` was re-pinned to
   `154.0.8037.92-1~deb13u1` (trixie-security; trixie's main archive carries an older one). A337.
2. **`RUN cargo run --release -p stvdy-server --fetch-embedder-model` was refused by cargo**
   (`unexpected argument '--fetch-embedder-model' found`) — cargo claims everything after its own
   options unless it is told otherwise. The flag is now passed through: `… -p stvdy-server --
   --fetch-embedder-model`. This step had never run, because the link error killed the stage before
   it.
3. **`tar xz -C /opt/pdfium --strip-components=1` put the library at `/opt/pdfium/libpdfium.so`
   while `PDFIUM_LIB_PATH` names `/opt/pdfium/lib/libpdfium.so`.** The release's layout is
   `lib/libpdfium.so` + `include/*.h`, so stripping one component moves the library and leaves the
   variable naming a file that does not exist — and A51 makes a **set-but-unloadable** path a
   **boot** failure, so this is a server that would not start rather than a disabled extractor.
   Found by `scripts/release-image.sh`'s own `test -f "$PDFIUM_LIB_PATH"` on the built image, and
   the fix is verified by extracting the pinned tarball (checksum matches the committed
   `PDFIUM_SHA256`) and confirming `lib/libpdfium.so`.
4. **The pin assertion could never pass, and it was written twice — in `scripts/release-image.sh`
   and in `.github/workflows/ci.yml`'s `image` job.** Both compared
   `chromium --version`'s output to `ARG CHROMIUM_VERSION`, and those are two different strings:
   the binary reports its **upstream** version (`Chromium 154.0.8037.92 built on Debian GNU/Linux
   13 (trixie)`) while the `ARG` is Debian's **package** version (`154.0.8037.92-1~deb13u1`). So
   the check was an unsatisfiable one wearing a green-looking name — the exact "a control that is
   configured but not enforced" failure mode this project has recorded before. Both now read the
   installed package version with `dpkg-query` (for `chromium` **and** `chromium-common`, which is
   the pair the Dockerfile pins together) and additionally assert the binary's upstream version is
   the prefix of it. The first version of that fix expanded `${Version}` in the container's own
   shell and answered `dpkg-query: error in show format: may not be empty string`, so the format
   string is escaped twice over and the comment says why.

5. **The `ARG CHROMIUM_VERSION` pin went stale between two builds of the same commit.** Debian
   superseded the browser — `154.0.8037.92-1~deb13u1` was current when the first image was built
   and `155.0.8059.39-1~deb13u1` replaced it a few hours later — and `trixie-security` **keeps only
   the current version**, so `apt-get install "chromium=154.0.8037.92-1~deb13u1"` answered
   `Version '…' was not found` and the build stopped. That is the pin working as designed (a
   superseded browser fails the build rather than silently changing every PDF), but it means the
   pin is a **weekly-rebuild chore, not a constant**, and a rebuild of an older commit will fail
   once its pin is superseded. Re-pinned to `155.0.8059.39-1~deb13u1` with the behaviour stated in
   the `Dockerfile`, and the PDF acceptance test is what proves the new browser still renders the
   six-page document. A337's "pinned base digest plus a weekly rebuild" is the mitigation this
   belongs to; a genuinely reproducible pin would need `snapshot.debian.org` URLs rather than the
   live archive, which is a larger change than this deployment.

6. **The runtime stage shipped no `config/`, so the server could not boot at all.** `AppConfig::from_env`
   resolves `PLANS_CONFIG` and `BILLING_POLICY_CONFIG` against the process's working directory
   (`crates/server/src/config.rs`), and the image's `WORKDIR` is `/app` — so `/app/config/plans.toml`
   was simply absent and the process exited with `loading plans config at
   /app/config/plans.toml: No such file or directory`. It took `--migrate-only` with it, so a
   deployment could not even migrate. **This is a boot failure, not a degraded feature**, and it
   survived because `prompts/` and `templates/` are `include_str!` and `migrations/` is
   `sqlx::migrate!` — everything else the server needs is compiled in, which makes the two
   filesystem-read config files look like they must be too. `COPY config /app/config` is the fix,
   and the Dockerfile now says which files are embedded and which are read.

7. **The documented `--migrate-only` command could never migrate.** Both `docs/OPERATIONS.md` and
   `docs/RELEASE.md` gave `docker run … -e DATABASE_URL="$DATABASE_URL" stvdy-server:<tag>
   --migrate-only`, and `scripts/release-image.sh` did the same. But the **image sets
   `APP_ENV=prod`** (its own `ENV`, not the caller's) and `--migrate-only` loads and validates the
   whole `AppConfig` before it touches the database, so it exited on `JWT_SECRET must be set to at
   least 32 characters when APP_ENV=prod` and then, once that was supplied, on `WEB_ORIGIN must be
   set when APP_ENV=prod` — without ever running a migration. The fix is that the migration step
   carries the same environment as the replica (`APP_ENV`, `JWT_SECRET`, `ENCRYPTION_KEY_HEX`,
   `WEB_ORIGIN`), and all three places now say so. **This one is the most dangerous of the six**:
   the step exits non-zero, but an operator who read "it exits 0 and binds nothing" as the
   contract and sequenced the replica anyway would have deployed against an unmigrated schema.

8. **The image had no `chromium-sandbox`, so the browser could not start — and the server hung
   rather than failing.** Debian ships the setuid sandbox helper as its own package and `chromium`
   does not depend on it, so the browser answered `No usable sandbox! If this is a Debian system,
   please install the chromium-sandbox package to solve this problem` and died. A338's whole point
   is that the sandbox stays on outside dev, so this is a missing prerequisite rather than a flag
   to add. **The failure mode is the worst of the eight:** the renderer is launched during boot
   (`crates/server/src/render/chrome.rs::launch_browser` → `Browser::new`), that call has no
   timeout, and a browser that dies mid-launch leaves the process alive at 1.6 GB RSS with
   `[chromium] <defunct>` in its process table, **no error logged and no port bound** — the check's
   symptom was a bare `curl: (7) Failed to connect to localhost port 18080`. `chromium-sandbox` is
   now installed and pinned with the browser. **The unbounded launch is fixed too**: `LAUNCH_TIMEOUT`
   (30 s) in `crates/server/src/render/chrome.rs` runs `Browser::new` on its own thread and gives it
   a deadline, so a browser that dies during startup now yields the absent-renderer policy this
   project already documents — the server starts, serves everything except PDFs, and the log names
   the launch — instead of a process that is alive, silent and listening on nothing. Verified both
   ways with a `CHROME_PATH` pointing at a script that sleeps forever: unbounded it prints
   `has been running for over 60 seconds` and never finishes, bounded it returns
   `could not launch the browser: the browser did not start within 30 seconds` in 30.00 s. The cost
   is one leaked thread per failed launch, stated at the function.

9. **The runtime stage copied the whole release profile, which made the image unbuildable on this
   host.** `COPY --from=build /src/target/release/ /opt/tests/` brought 6.4 GB of *other* test
   binaries, their rlibs and cargo's `build/`/`.fingerprint/` bookkeeping into a runtime image where
   nothing runs any of it. The acceptance run needs one binary, and `tests/pdf.rs` is
   self-contained — no `include_str!`, no `File::open`, no `CARGO_MANIFEST_DIR` — so `deps/pdf-*`
   plus the build's shared objects is the whole requirement. The cost was not only image size:
   BuildKit holds the build stage's `target/` **and** the runtime image's copy at once, so the peak
   was ~37 GB and a 45 GB host with ~9 GB of OS and other images **ran out of disk mid-export**
   (`copy file range failed: no space left on device`), twice. Now the image carries
   `/opt/tests/deps/pdf-*` and `deps/*.so` only, and A337's wording was narrowed to match.

10. **The documented `docker run` stanza had no `--security-opt seccomp=unconfined`, so the sandbox
    could not start even with `chromium-sandbox` installed.** Docker's default seccomp profile
    blocks the namespace the renderer's sandbox moves into: the browser answers `Failed to move to
    new namespace … errno = Operation not permitted` and `Zygote process exited prematurely with
    exit code 1`. `--shm-size` and `--tmpfs /tmp` are the sandbox's *resources*; this flag is its
    *permission*, and it was missing from every one of the four places the container is started —
    `docs/OPERATIONS.md`, `docs/RELEASE.md`, `.github/workflows/ci.yml`'s `image` job and
    `scripts/release-image.sh`. All four now carry it, `deploy/staging/docker-compose.staging.yml`
    has `security_opt: [seccomp:unconfined]`, and `--cap-add SYS_ADMIN` is recorded as the broader
    alternative the setuid helper path needs where unprivileged user namespaces are unavailable.
    Confirmed both ways: with the flag, Chromium logs `Activated seccomp-bpf sandbox for process
    type …` and renders; without it, the zygote dies. **The failure mode was the hang described in
    item 8** — no error, no port bound — which is what made the image look unbuildable rather than
    unrunnable; `LAUNCH_TIMEOUT` (item 8) is what now makes it a missing renderer instead.

11. **A partial build context produced a runtime 500, and the migration step could not repair it.**
    I rsynced `crates/` before a rebuild but not `migrations/`, so the image carried the session's
    new `workspaces.course_code` query without `0038_workspace_course_fields.sql`. `sqlx::migrate!`
    embeds migrations at **compile** time, so `--migrate-only` ran *that image's* embedded set, which
    did not contain the migration, and the schema stayed behind the code. The symptom was narrow and
    misleading: `/health` green, signup and login green, and
    `GET /workspaces` → `500 internal error: listing workspaces: error returned from database:
    column w.course_code does not exist`. `scripts/staging-sync.sh` now owns the sync-and-build step
    with the full compiled-in list, and `docs/OPERATIONS.md` § Staging names the trap and this
    symptom.

What the staging environment itself is:

- **Both firewall layers are open, and the stack is reachable from outside.** The host's own layer
  admits 80 and 8080 (persisted in `/etc/iptables/rules.v4`, with `iptables-persistent` installed
  and enabled, and `ufw` agreeing) and the Oracle **VCN** now carries the ingress rule for TCP 80
  and TCP 8080 from `0.0.0.0/0`. **The two layers fail identically from outside**, which is why
  the pair is worth naming: with only the host layer open, everything is listening, `curl` works on
  the machine, and every port is still closed to a laptop.
- **Verified end to end through a real browser over the public origin, not just with `curl`.**
  `http://129.225.10.139/` → the sign-in screen; *Create an account* → `POST /auth/signup` **201**;
  the verification link read out of the mail sink and opened → `POST /auth/verify-email` **200** and
  "Email verified"; sign in → `/workspaces` with `GET /settings`, `/tasks`, `/events`, `/workspaces`
  all **200**; *New workspace* → `POST /workspaces` **201**; sign out and back in → the workspace is
  still listed. Every request is cross-origin (`Origin: http://129.225.10.139` against the API on
  `:8080`), so this is also the CORS path, and the preflight answers
  `access-control-allow-origin: http://129.225.10.139`.
- **TLS, R2, billing and Google are deliberately absent** there, and the table in
  `docs/OPERATIONS.md` § Staging says why for each. None of it is a gap in the production
  checklist: the host is a one-month trial, so nothing a third party was told about may point at it
  (A45). **The one that needs saying out loud is TLS**: the host has no DNS name, so no public CA
  will issue it a certificate, and `POST /auth/login` therefore carries a password in cleartext over
  the public internet. Treat `http://129.225.10.139/` as an operator-only surface — do not hand it
  to a student, and scope the VCN ingress rule to one source address if it has to be shared.

## Credentials needed as the later phases land

| Phase | What |
|---|---|
| 10 (calendar) | A Google Cloud OAuth client for **Calendar** (id, secret, registered redirect URI). The same client serves sign-in-with-Google from Phase 1, but the calendar connection asks for `calendar.app.created` + `calendar.events.readonly` and may want its own client |
| 13 (payments) | **Done, and re-cut for the real gateway.** The trait, the pure billing math, the checkout, the one `apply_paid` every ingress shares, the periods, BYOK, the margin report and the Jakarta money gate all shipped. **B1 then resolved and the wire was rewritten against the actual gateway (2026-10-13):** the bridge is polled rather than pushing, so the callback route, its three signature schemes and its two secrets are deleted; it assigns a unique payable amount, so `payments.payable_amount_idr` was added and the settlement check reads it; and the checkout screen's own poll became the fast ingress, because without it a student who had paid waited five minutes for the sweep. One real hole found and fixed earlier and still closed: the checkout screen never consulted `billing_policy`, so a phone build had a purchase screen |
| 14 (mobile) | An Apple Developer account (APNs `.p8` key + key id + team id) and a Firebase project (`google-services.json` + an FCM v1 service account). The push-receive, share-extension and background-scheduler steps need a **physical device** to verify; they degrade to a simulator for everything except the push and background cases |
| 15 (release) | An Apple Developer ID certificate for notarization, a Windows code-signing certificate, and a **production host** (A45: the current VPS is a 1-month trial and must never be production). Also a Cloudflare R2 bucket + credentials for production object storage (A15) |
| 15 (release) | A decision on the updater signing key: where it lives and who holds it. A signed manifest whose key sits on a laptop means an update can be forged |

## The one capability nobody has verified, and it is the one that decides the mobile approach

**Every native probe in Phase 1's capability spike is recorded as NOT ATTEMPTED, and Phase 14 built on top of it anyway.** The roadmap's own Phase 1 text says this explicitly — *"do not defer to Phase 14 … Discover that in Phase 1, not after 13 phases"* — and the reason is that the answer can force a different plugin, a hand-written Swift/Kotlin shim, or a framework change that would have to be asked about before more is built on it.

The probes and their recorded state (`docs/ARCHITECTURE.md:344`, `docs/superpowers/plans/2026-10-11-phase-1-auth-devices-ingest.md:622`):

| Probe | State | What it decides |
|---|---|---|
| APNs token registration (iOS) | **NOT ATTEMPTED** | Whether the pinned community plugin (`tauri-plugin-notifications`, A310) is enough or a custom Swift plugin is needed |
| FCM token registration (Android) | **NOT ATTEMPTED** | The Kotlin equivalent, plus whether the Gradle plugin and `google-services.json` wiring is right |
| Foreground push receipt + tap-to-open, including cold start | **NOT ATTEMPTED** | Tap-to-open is the only guaranteed JS surface (A316), so this is the whole of the reliable push UX |
| iOS "Open in…" document intake | **NOT ATTEMPTED** | Whether `RunEvent::Opened` delivers on a **cold** start under the scene lifecycle, which the capability matrix flags as unverified |
| iOS share-sheet row | **NOT ATTEMPTED** | Whether the Share Extension + App Group path (A312) works, and whether the entitlement is obtainable |
| Android `content://` share/import | **NOT ATTEMPTED** | Whether a concrete `mimeType` in `fileAssociations` delivers a URI the fs plugin can read |

**What this means for you.** The code for all of it is written and its pure halves are asserted, but **none of the six has ever run on a device**. The Phase 0 finding already on record is the warning shot: the official `tauri-plugin-notification` **provably cannot receive remote push** (its iOS delegate skips `UNPushNotificationTrigger` and its Android manifest registers no FCM service), which is exactly the kind of discovery the spike existed to make early. So the honest state is: **the mobile native work is unverified, and a failure here is a design change rather than a bug fix.** Do the spike before building anything further on it — an Apple Developer account and one physical device answer all six rows, and a simulator answers four of them.

## Verified green so far

Every phase's exit gate is recorded in its own plan file, and each phase's working notes are in
`.superpowers/sdd/<plan>/progress.md` (git-ignored scratch, on disk only). **One exception worth
knowing before you trust the table below:** Phase 4's plan had no `## Execution notes` section at
all, so its gates were never recorded anywhere — its tests are covered by the workspace run, and
its two roadmap gates (5 and 6) had no measurement in any document. **Both are now closed
(2026-10-12):** gate 6 was performed all along by `flashcards.rs::a_deck_generates_cards_linked_to_their_chunk`,
and gate 5 has now been performed on the study screen for the first time, with one defect found
and fixed — see the plan's new `## Execution notes (Phase 4, as built)`. **One phase still has
never measured its accessibility gate — 0**, and that is its own finding, below.

**The workspace quality gate — `cargo fmt --check`, `cargo clippy --workspace --all-targets -- -D
warnings`, `cargo test --workspace` — is green, and this is the first time in the project's
history.** `cargo test --workspace --no-fail-fast` exits **0** with every target ok; the last
blocker was Phase 11's own exit-gate suite, which had been written, committed and left unrun at
14 passed / 5 failed until `9f1d6ae` fixed it. The shell's trio is green too, and it is run with
`--workspace` now, because `src-tauri/plugins/mobile` is a workspace member that clippy and test
were silently skipping. **The counts, re-measured 2026-10-12 after the workspace grew:** core
**396**, `stvdy-server` **706**, `stvdy-sync` **33**, the shell **71** (test and doc-tests), the SPA
**632** across 69 files — **workspace total 1,135** (396 + 706 + 33), with the shell's 71 as a
separate command because `src-tauri` is outside the workspace. The earlier figures (382 / 563 / 33 /
71 / 604) were right when written and have moved with the phases. **Two arithmetic traps the older
line had, both worth not repeating:** its "about 1,650 assertions" mixed the workspace and the shell
legs into one number, and a later note in this session read a workspace+shell total of 1,205 as if it
were the workspace alone. Keep the two legs apart. Of which
roughly a third need a live Postgres and create their own scratch database per case. The short
version:

| Phase | Status |
|---|---|
| 0–5 | Done before this session; verified green again here |
| 6 (reports, cheatsheet, PDF) | **Done.** Implementation, a review round with one Critical fix (an SVG attribute-value injection that reached a printed PDF), the three roadmap gates measured in a real browser (four defects found and fixed), the smoke run passing |
| 7 (entitlements, credits, limits) | **Done.** The engine, the sweep, the ledger with a mutation-proved concurrency test, the period, the abuse controls, the ingestion formats, the tamper suite (doc-driven), admin, the SPA, the gates (one defect found: a 70×19 upgrade link), the smoke run |
| 8 (sync engine) | **Done.** A consumer-visible defect was found and fixed: a pull shipped only the *first* change of a row, so every later change arrived as `data: null` — which the client applies as a **delete**. Gates measured; one WCAG Level A failure found (the document's language attribute never followed the UI language) and fixed |
| 9 (tasks, events, Today, timer) | **Done.** Four defects found and fixed, all "two write paths disagreeing" — a push delete that did not cascade, a restore that did not restore the cascade, a calendar-owned event writable through push, and a local apply that did not cascade. Three `QuickCapture` defects found by measurement and fixed |
| 10 (calendar) | **Done.** Two plan-level errors corrected (the `rrule` bound, and the migration numbering); two more 200%-text layout defects found by the gate and fixed; a two-clock defect in the poll diagnosed from the rows and fixed |
| 11 (syllabus, planner) | **Done.** Two real client defects found by driving the wired process rather than by a unit test, and **four of the slice's own assertions were wrong about the product** — the strongest argument in the project for exercising the running app. **And its exit-gate suite had never been run**: `cargo test -p stvdy-server --test syllabus` was written, committed, left out of the exit checklist's result column, and found red on committed code (14 passed / 5 failed) when the workspace gate was finally run. Four of the five were the same mistaken premise — the test author read `item.index` as a position in the *sorted* review list, while the contract (`crates/core/src/generation/syllabus.rs:109-113`) defines it as the position in the **reply**, across the module's field order; the product was right in every case and the assertions were corrected with the assumption named. The fifth was the test harness's `drain_jobs`, which cannot assert an empty queue for a deliberately failing retryable job. Now **19 passed / 0 failed** (`9f1d6ae`) |
| 12 (agent, reminders, dashboard) | **Done.** One real **cross-tenant write** found by the slice's own injection test and fixed: a task or event could be filed into another account's workspace. The injection test's three-part anti-vacuity proof is the phase's best evidence. **And one defect that cost money, found only by running the suite:** a turn refused for a spent balance set its `charged` flag *before* returning the refusal without recording why, so the next call in the same turn read that flag as "already paid" and **ran its second tool call for free** — a turn that could not afford `agent_turn` still proposed. The first diagnosis (a `limit_reached` refusal falling through a one-string match) was real but partial, and fixing only the string would have left the free call live |
| 13 (payments) | **Done.** The trait with a real-HMAC mock, the pure billing math, the gateway behind one mapping file, the checkout, the webhook with two separately-tested idempotency defences, polling that shares one `apply_paid`, the periods, BYOK, the margin report with its tripwires, the Jakarta money gate, and the smoke run green. **One real hole found and fixed**: the checkout screen never consulted `billing_policy` and the route was registered unconditionally, so a phone build had a purchase screen reachable by a typed URL. **Two live product defects found by the green run itself**: `POST /billing/checkout` was a 500 for every caller (an INT4 amount decoded as i64), and a paying student kept free limits because `users.plan_id` was never written on an upgrade |
| 14 (mobile) | **Done, except the native work.** The bundle-absence proof is now a build-time exclusion rather than a promise: `TAURI_ENV_PLATFORM` keys the store build, every build stamps its own `stvdy-build-target`, and the check refuses anything but `store` — measured, **0** occurrences of `billing/checkout` in the 764,625-byte store chunk against **5** in the 778,765-byte web one, failing closed on an empty directory, an absent stamp and a `full` stamp. Gate 5 now runs at **both** viewports for the first time (13 surfaces, 80 combinations at 390×844 and 80 at 1365×900); six phone-layout defects and three desktop findings came out of it. The native work (push receive, share extension, background scheduler) needs **your** accounts and a device |
| 15 (release) | The release gate, the documents, the licensing artifacts and the store checklists are done; the verification script was exercised against a real signed fixture and found two bugs in itself. **The server image is not built anywhere yet, and this row said it was.** The `image` job had never reached its linker: `COPY .sqlx` copied a directory the repository does not contain, so the build failed on the first line of the stage and hid everything behind it (fixed 2026-10-12). What it fails on now is real: `cargo build --release` inside the container cannot link `ort-sys`'s prebuilt ONNX Runtime, which needs glibc ≥ 2.38 (`__isoc23_strtoll`, `__isoc23_strtoull`, `_M_replace_cold`, all referenced from `libort_sys-*.rlib`) while the build stage is `bookworm` (glibc 2.36). Signing, notarization and the store submissions need **your** certificates and a production host |

## Things found that you should know about

- **A live SSRF guard that did DNS work instead of refusing, and the test that proved it was red on committed code.** `guarded_get` read the request host with `Url::host_str()`, which **keeps an IPv6 literal's brackets** (`[::ffff:169.254.169.254]`). `resolve_public` then tried `parse::<IpAddr>()` on that string, failed on the brackets, and fell through to a resolver — so the fetch-time guard, the one whose own doc comment calls itself "the thing that must not be duplicated", did network work where it had to refuse, and answered with a `Provider("resolving …")` error that reads like an outage. `check_fetchable_literal` had already been fixed for exactly this reason and says the suite caught it; the fix landed in the courtesy check and not in the guard. `crates/server/tests/ssrf.rs::the_guarded_text_fetch_refuses_a_private_host_and_a_non_http_scheme` asserted the property correctly, needs no network and no stub, and was **7 passed / 1 failed on committed code** — a test nobody ran, not a test that was missing. Fixed (`d23232c`), now 8/0.
- **A gap in the build's own safety net, found by hand:** the root workspace excludes
  `src-tauri` and `src-tauri` is its own workspace root, so `[workspace.lints]` in the root
  **never reached it** — the project's "no `unwrap`/`expect` in non-test code" rule was
  unenforced in the one crate that holds a keychain and a SQLite cache. Fixed: the shell declares
  the lints, and the one real violation (an `expect` on the event loop's result) now logs and
  exits non-zero, so a supervisor sees a failure rather than a process that vanished.
- **A defect in the roadmap itself, found by Phase 13's plan:** the margin report's attribution
  was specified as a join on `users.plan_id`, which is wrong in the one month the report exists
  for (it holds only the *current* plan, so a converting user re-attributes their own earlier
  free-tier cost). Corrected in the roadmap, with a test that seeds a June-free/July-paid user.
- **A defect in Phase 10's plan, found by its own implementation:** `rrule` 0.14 reports
  `limited == true` when the collection merely *reached* the limit, so the planned bound would
  have marked a rule with exactly 500 occurrences as truncated — a connection that looks broken
  while it is fine. Corrected, with two cases that have the same occurrence count and opposite
  answers.
- **Three defects in the app's `QuickCapture`, all found by measuring rather than looking — all three now fixed and regression-tested.** Escape did not close the draft or return focus, the Due line rendered a raw UTC instant (so a task due 06:00 in Jakarta named the wrong day), and the ISO token pushed the page 34px wide at 200% text on a phone. `QuickCapture.tsx:47-79` handles Escape and hands the caret back to the composer, the due line goes through `formatInZone` (the same helper the screen uses), and the token carries `min-w-0 break-words`; `QuickCapture.test.tsx` asserts all three, with the Chromium measurements in the plan's gate notes. Kept in this list rather than deleted because the *class* — a defect invisible until something measures at a second text scale — is the part worth carrying forward.
- **A WCAG Level A failure in the SPA, fixed.** `index.html` is a static `lang="en"` and nothing updated it, so a screen reader pronounced the entire Indonesian UI with an English voice. Found by Phase 8's gate. **Fixed, and the fix is a runtime setter rather than the attribute:** `app/src/i18n/index.ts`'s `applyDocumentLanguage` runs once for the resolved language *before* the first render and again on every `languageChanged`, with four cases in `i18n.test.tsx`. `index.html`'s static `lang="en"` stays deliberately — it is what a screen reader gets for the fraction of a second before the module runs, and changing it to `id` would mislabel the English default.
- **Every piece of copy the server generates for a student is English-only, in an Indonesian product — and one of them is a notification the student gets at 09:00.** The spec's line 93 requires "English and Bahasa Indonesia from day one … no hard-coded strings" for the SPA, and the SPA meets it. The server does not, and nothing checks it:
  - `crates/server/src/auth/email_verify.rs:83` — subject `"Verify your Stvdy email"`, body `"Confirm this address to finish setting up your account."` This is the email that gates every AI action (A43), sent to a student who may have chosen Indonesian.
  - `crates/server/src/jobs/handlers/renewal.rs:338-367` — `copy()` builds `"Your {plan} plan {when}"` and a two-sentence body, and this is the text that arrives **as a push notification and as the in-app feed row** (the comment says so: "the text the student sees in the app and the text that arrives as a push are the same sentence from the same row").
  - `crates/server/src/billing/apply.rs:516-589` — the receipt's subject and body, still English-only. **Its second defect — the one that is not a judgement call — is now fixed, and it was real:** the activated/extended arm wrote `period_end.to_rfc3339()` into the student's email, so what arrived was `Your pro plan is active until 2026-11-01T17:00:00+00:00` — a **raw RFC3339 instant in UTC**, for a product whose own Jakarta gate (`crates/server/tests/billing_jakarta.rs`) exists to assert that every student-visible date renders through `chrono_tz` in the reader's zone. For a Jakarta student that instant is **2026-11-02 00:00 WIB**, so the email named the wrong day by the same mechanism already found twice in this project (the `QuickCapture` Due line, and `plan.periodEnd`, which the Jakarta test asserts correctly). The plan name was `plan.as_str()`, so the email also said "pro" rather than "Pro". **Both halves are fixed**: the date renders through the account's own zone (`users.timezone`, A203 — the same column the reminder and planner maths read) in `%-d %B %Y`, the shape `renewal::copy` already uses, and the name comes from `renewal::plan_name`, now `pub(crate)` so there is one capitalisation rule rather than two. Two cases in `billing_jakarta.rs` pin it (`the_receipt_names_the_students_own_day_and_plan`, and `the_receipt_falls_back_to_the_default_zone` for a blank zone), and each was shown failing against the pre-fix form. The **English-only** half of this bullet stands unchanged: the receipt is still one English sentence, and that is the product decision below.

  **The mechanism, and why the tree cannot express the fix as written.** The server does send a key — `renewal::PLAN_ENDING_KEY = "reminder.planEnding"` goes into the `send_push` payload beside the rendered sentence — and the intent is stated in the code: "so the SPA's feed renders the same reminder in either locale from one key rather than re-deriving the copy from the numbers". But `app/src/i18n/locales/en.json` has **no `reminder` block at all** (only `reminders.local.*`, which is the client-computed task/event copy), so that key resolves to itself, and the client's own guard is what hides it: `useLocalNotifications.ts:90`'s `feedKey` returns `null` when `subject_id` is `null`, and the renewal row's `subject_id` **is** `null` by construction (`renewal.rs:257`), so the row is skipped before any copy is looked up. **A renewal reminder therefore renders nowhere on the client, and the rendered English sentence the server built is what the student actually receives.**

  **Also worth knowing:** `user_settings.language` exists (`migrations/0028_tasks_events_settings.sql:30`, `check (language in ('en','id'))`) and is per-account, so the lookup the fix needs is already modelled. What is missing is a copy table in two languages and the read. **I have not implemented it**: it is a product decision as much as a task (is an English receipt acceptable? a receipt is a financial document, and the answer may differ from the answer for a verification email), and it is new scope rather than an unexecuted phase. Recorded here with the evidence so the decision is yours.
- **All three phases with an accessibility gap are now closed, and the last one closed a product gap as well as a gate.** **Phase 4's** was the screen a student uses most: four surfaces of `/study/:deckId` (card unrevealed, revealed, source open, graded by the `3` key) were added to the shared walk and measured at both viewports, both themes, both scales — phone **160 targets, 0 under 44px, 304 text pairs, 0 below, lowest 6.29:1**, desktop **176 targets, 0 under 32px, 336 pairs, 0 below**; 0 clipped, 0 overlapped, page width exact at both. **One defect found and fixed:** the header's "Back to the workspace" was a bare `.link` — a body-copy class used for a standalone control — so it painted its line box at **151.3×21** (**302.5×41** at 200% text) against the 44px floor, the same class of defect Phase 7's gate found on its upgrade link. Fixed with `inline-flex min-h-11 items-center`, re-measured at **151.3×44** / **302.5×88**; the distinction is now written beside `.link` in `app/src/styles.css`. Regression test proven to fail without the fix. **Phase 0's** was the shell, measured through the walk rather than a Phase 0 probe: every one of the 24 surfaces is rendered *inside* the shell, so its chrome is in the control set on all 96 combinations — tab bar items **78×44** at phone width, sidebar items **215×40** (above the spec's 32px desktop floor), the skip link the first Tab stop at 16.19:1, and 1,188 Tab stops with 0 unreached and 0 without a ring. Its pre-auth screens (`/signin`, `/signup`, `/verify`) are **not** in that walk — Phase 1's own walk is their evidence. **Phase 11's is now measured too, and it was the one where the gate's gap *was* the product's:** leg (e) had gone unmeasured because the walk could not **reach** the screens — the only `/syllabus` link in the app was inside the planner's empty-goals state, so a student with any goal could not open the syllabus import at all, and neither nav has a planner item, so `/planner` itself was reachable only by typing a URL. Fixed with the reciprocal half of the spec's `Plan | Tasks` control and by rendering the planner's import link with the goal list; the three surfaces then measured **132 targets, 340 text pairs, 0 under 44px, 0 below requirement, 0 clipped, 0 overlapped, page 390/390**. **Two more real defects came out of it:** the review screen's root lacked `w-full`, so at 200% text the page was **680px wide in a 390px viewport**, and its `Keep this` checkbox was **61.3×22** because the input and its label were siblings rather than the input being inside the label. `docs/ARCHITECTURE.md` § Accessibility gate is now the **running record**, with a table row per phase, so "has this been done" is answerable in one place — which is the finding these closures produced. The one layer no headless browser reaches is native (the mobile and desktop shells themselves); `docs/RELEASE.md`'s store checklists type that `user`.
- **The shared harness used to measure three of gate 5's five criteria; both of the gaps are now closed.** It measures contrast, target size and 200%-text clipping; **keyboard reach with a visible focus ring (a)** is measured by the walk's keyboard leg on every surface (Phase 14), and **state-by-colour-alone (d)** is now measured on **two** surface families — the six checkout states, and Phase 4's study screen, whose revealed state declares its four grade labels as `words` in `SURFACES` so the assertion is on rendered text rather than a tone. What remains unmeasured for (d) is every *other* surface: the only way to infer "state from colour alone" on a screen whose state is not declared is a computed-colour difference between siblings, which produces findings nobody can act on. `var/gate5-keyboard.json` (three report screens, (a)-only, not reproducible from any committed command) is superseded for every surface the leg now walks.
- **Gate 5's desktop leg had never run, for any phase, and nobody could tell.** The shared walk's
  steps name the phone tab bar's labels (`nav.work` = "Work") while the desktop sidebar reads
  `nav.workspaces` = "Workspaces", so `VIEWPORT_WIDTH=1365` died on `FAIL: no nav a labelled
  "Work"` after the first surface. Every phase's desktop-width accessibility evidence was either
  taken by a private probe or absent. Fixed in `bc7eed8`; the first real run found the sidebar's
  nav items at **215×40** (under 44px in height on every surface, both themes) and a `minmax(200px,
  1fr)` grid floor that does not grow with the text.
- **A class of layout defect that only one measurement caught, and it is the one that matters
  most.** The dashboard scrolled sideways at 390px **at 100% text** (423 > 390, 15 elements past
  the edge) — the first defect in the whole series visible at *normal* size, and a different
  mechanism from the rem-doubling one that every other finding belonged to: a `truncate`'s
  `white-space: nowrap` min-content propagating up through a flex column until an *ancestor*
  reports the overflow, where the element that is too wide reports none of its own. `min-width: 0`
  on the root is **inert in the cross axis** (measured: still 423/390); `w-full` is what makes the
  width definite. Both scales are now part of the pass condition.
- **The absence gate was asserting the wrong thing about the wrong artifact, and the fix changed
  the remedy.** Phase 13's slice first reported that `scripts/check-no-purchase-ui.mjs` "inspects
  the web build while calling it the mobile build". That was wrong: `frontendDist` is
  `../app/dist` with no per-platform override, so those bytes **are** what a phone loads and the
  check was reading the right artifact and reporting a real violation. What was missing was a
  **build-time exclusion** — the route was chosen at run time, which does not remove bytes. Now
  `TAURI_ENV_PLATFORM` keys the store build so the phone bundle lands where `frontendDist` points,
  every build stamps its own `stvdy-build-target`, and the check refuses anything but `store`:
  **0** occurrences of `billing/checkout` in the 764,625-byte store chunk against **5** in the
  778,765-byte web one, and it fails closed on an empty directory, an absent stamp and a `full`
  stamp as well.
- **A screen that rendered five of its twelve field labels as raw locale keys**, caught by the
  Indonesian browser render and by nothing else: the card resolved labels from the tool schema's
  argument names, and neither the i18n contract test (which marks every key under a prefix as
  used) nor the card's own test (which asserts the *value*) could see a missing label.
- **A locale key that could not be rendered at all:** `reminder.planEnding` has no entry in either
  locale file and no renderer in the SPA — the renewal writes `subject_id = null` and
  `feedKey` returns `null` for such a row. Found by trying to assert the Indonesian renewal
  reminder, and recorded as a contract the tree does not implement rather than papered over.

## Things worth knowing about the plan set

All sixteen phase plans now exist and are committed. Two of them were corrected against the
tree before execution rather than during it (Phase 6 needed five corrections, Phase 7 five
more), and two rounds of assumption-number collisions were resolved by renumbering before any
row was written. The reserved blocks are now: 6 → A119-A139, 7 → A140-A163, 8 → A164-A189,
9 → A190-A229, 10 → A230-A249, 11 → A250-A269, 12 → A270-A289, 13 → A290-A309, 14 → A310-A329,
15 → A330-A354.
