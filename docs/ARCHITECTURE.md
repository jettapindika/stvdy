# Architecture

Filled incrementally; each section names the phase that completes it.

## Layers

```
app/  (one React SPA: web build + Tauri frontend for macOS/Windows/iOS/Android)
  |  HTTP + SSE
crates/server  (Axum: routes, auth, sync, job worker, integrations)
  |  traits
crates/core    (pure logic: chunking, retrieval, citations, SM-2, planner,
                entitlements, prompt composer, document renderer)
crates/sync    (wire DTOs shared by server and the native shell)
src-tauri      (native shell: keychain, local cache, plugins)
```

`crates/core` has no web, database or IO dependency, so its logic is unit-testable
against a fixed clock. That is what makes the planner, SM-2 replay and the
entitlements engine deterministic and cheap to test.

## Provider traits

All external capabilities sit behind traits in `crates/core/src/traits/`, so a
provider is configuration rather than code. Phase 0 defines the contract; each
phase adds its first implementation.

| Trait | First implementation | Phase |
|---|---|---|
| `LlmProvider` | OpenAI-compatible `/chat/completions` (streaming + tools) | 2 |
| `Embedder` | `fastembed` `MultilingualE5Base`, 768 dims, `query: `/`passage: ` prefixes applied by the impl (or an HTTP embedder) | 1 |
| `SearchProvider` | Tavily | 3 |
| `PaymentProvider` | `qris_gateway` | 13 |
| `PushProvider` | APNs (token auth) + FCM v1 | 12 |
| `Storage` | local filesystem (`STORAGE_BACKEND=local`, signed URLs served by this server) or S3-compatible (`aws-sdk-s3`; MinIO fork in dev, Cloudflare R2 in prod per A15) | 1 |
| `Mailer` | `lettre` over SMTP, or a dev file sink writing `var/mail/*.eml` | 1 |
| `Clock` | system clock; fixed clock in tests | 1 |

## Job worker

One in-process worker per server process, backed by a single `jobs` table (A10:
no Redis). An enqueue wakes the worker through a `Notify`; a 5-second poll
covers `run_at` scheduling and a delayed retry.

The claim is a single statement:

```sql
update jobs set status = 'running', attempts = attempts + 1
 where id = (select id from jobs
              where status = 'queued' and run_at <= now()
              order by run_at
              for update skip locked
              limit 1)
returning id, kind, payload, attempts;
```

`SKIP LOCKED` is what lets several workers, or several instances, run without
contending on the same row.

A failure is classified, not just reported. `Failure::transient` (a provider
hiccup, a timeout, a storage error) is retried up to `MAX_ATTEMPTS = 5` with a
`2^attempts`-second backoff, capped at `attempts.min(10)` so a long-lived row
cannot schedule a retry arbitrarily far out. `Failure::permanent` (a missing
pdfium library, an unparseable document, an unsupported source kind, an unknown
job kind) marks the job `failed` immediately. Retrying a permanent failure
leaves the source `processing` for half a minute and burns the queue behind it;
the stored `error` is what the client shows, so the message must be actionable.
A handler that panics is caught (`catch_unwind`) and treated as transient, so a
job row can never be left `running` by a panicking handler.

A boot sweep (`requeue_orphans`) requeues any row still `running`, because a
process killed mid-job would otherwise park that row and its source forever.

`dedupe_key` is a nullable column with a **partial** unique index
(`where dedupe_key is not null`). It exists so singleton recurring jobs
(`monthly_grant`, `compact_change_log`, `poll_payment`) can be enqueued from a
boot path and a duplicate is a no-op rather than a double-run. The consequence
to remember is on the writer: `on conflict (dedupe_key) where dedupe_key is not
null do update ...` must repeat the `WHERE` predicate, because Postgres will not
infer a partial index as a conflict target without it.

## Config

- `config/plans.toml`. Plans, limits, credit costs, provider cost ceilings.
  Loaded and validated at startup; `plans.toml` is the authority for
  entitlements, and the `plans` table is a mirror for FK integrity and auditing.
- `config/billing_policy.toml`. Per-platform purchase policy. `validate()`
  rejects a mobile entry that enables purchase UI, prices, or a CTA, so the file
  cannot be edited into an app-store policy violation.

## Trust boundary

**The client is untrusted; the server is the only enforcement point.** Everything
left of the API, the Tauri shell, the web SPA, and a scripted client holding a
valid token, is assumed hostile and possibly patched. The only thing a client
contributes is *authenticated intent* ("this user wants to send this message in
this workspace"); whether that intent is permitted, affordable, or well-formed is
decided on the server, per handler and per sync mutation.

`docs/THREAT_MODEL.md` is the normative statement of this boundary: the assets, the
attacker classes, what a modified client can and cannot do, the accepted residual
risks, tamper resistance by platform, and the per-phase verification obligations
that follow from it. It is linked here and from `README.md` so it cannot drift out
of the document set, and the phases that implement against it are Phases 7
(`tamper.rs`), 8 (cross-tenant isolation) and 13 (payment trust).

## Native capability matrix

See `docs/research/2026-10-11-tauri-2-capability-matrix.md` for the verified
platform matrix, the items needing custom Swift/Kotlin, and the items that are
impossible (background JS execution, guaranteed periodic background sync).

Decisions recorded from that research:

- **Secure storage**: `keyring-core` 1.0.0 plus a per-platform store wired by
  `cfg`. The `keyring` crate's defaults are desktop-only, so iOS/Android need
  their own store crates. Android needs **no** Kotlin shim: android-activity
  already initialises `ndk-context`, and a second call aborts
  (`assert!(previous.is_none())`).
- **Push**: no official Tauri push plugin exists; the official notification
  plugin cannot receive APNs/FCM. Phase 14 owns this.
- **Scheduling**: the official notification plugin schedules local notifications
  that fire while the app is closed and offline on mobile, but ignores scheduling
  entirely on desktop. Desktop reminders therefore come from server push and
  in-app delivery, with an in-process timer while the app runs.
- **Updates**: `tauri-plugin-updater` needs a signing keypair and a release
  endpoint, so it is registered in Phase 15 rather than Phase 0. Registering it
  with placeholder config panics at startup.

## Native prototypes

### Phase 0. Keychain round trip (macOS): PASS

`src-tauri/src/keychain.rs` has a test that stores, reads, overwrites and
deletes a refresh token through the OS keychain. Result on macOS 27 (arm64):

```
test keychain::tests::keychain_round_trips_a_refresh_token ... ok
```

No keychain prompt appeared for the locally built binary. The shell also builds
and launches: `tauri build --debug --no-bundle` produces
`src-tauri/target/debug/stvdy-shell`, which runs and holds a window.

### Phase 1. Auth, devices, ingestion

The native half of Phase 1 is the deep-link round trip and keychain token
storage; the server half (auth, devices, workspaces, sources, ingestion) is
covered by `docs/API.md` and the provider traits above. The acceptance question
was "does a `stvdy://` URL reach the SPA, on both cold start and a running
instance, and does the SPA get the refresh token into the OS keychain through the
adapter". Answer: the URL reaches the shell's classified intent on both cold
start and a running instance on macOS (PASS); the classification and adapter
halves of the keychain path are PASS, and the webview-to-Rust IPC hop was not
observed end to end (see the keychain line). The iOS simulator build is BLOCKED
and is recorded as such, not upgraded.

`src-tauri/src/deeplink.rs` holds the pure classifier: `stvdy://auth/callback`
with a non-blank `code` and `state` becomes an `auth_callback` intent,
`stvdy://oauth/calendar` becomes a `calendar` intent, and everything else
(foreign scheme, unknown route, missing or blank `code`/`state`) becomes an
`ignored` intent carrying a coarse reason. It never panics, and it logs only the
action label, never the URL, because a query string can carry an OAuth `code`.
The shell emits `{action, payload}` on the `stvdy://deep-link-intent` event and
answers `deep_link_current` with the same shape, so no module outside
`app/src/platform/` parses a URL. `DeepLinksApi.current`/`onOpen` in
`app/src/platform/types.ts` now take `DeepLinkIntent` instead of `string`.

#### Item 1. Deep-link classification unit tests: PASS

```
cargo test --manifest-path src-tauri/Cargo.toml
```

```
running 12 tests
test deeplink::tests::a_foreign_scheme_is_ignored ... ok
test deeplink::tests::an_ignored_intent_never_carries_the_url ... ok
test deeplink::tests::an_unknown_route_is_ignored ... ok
test deeplink::tests::classifies_a_calendar_return ... ok
test deeplink::tests::classifies_an_auth_callback ... ok
test deeplink::tests::malformed_input_never_panics ... ok
test deeplink::tests::missing_or_blank_credentials_are_ignored_with_a_reason ... ok
test deeplink::tests::percent_decodes_the_code_and_state ... ok
test deeplink::tests::serializes_to_the_action_payload_shape ... ok
test deeplink::tests::the_action_label_is_loggable_and_url_free ... ok
test keychain::tests::secure_store_availability_matches_the_platform_matrix ... ok
test keychain::tests::keychain_round_trips_a_refresh_token ... ok

test result: ok. 12 passed; 0 failed; 0 ignored
```

`an_ignored_intent_never_carries_the_url` asserts the serialized `ignored`
intent contains only `{"action":"ignored","payload":{"reason":"not_stvdy_scheme"}}`,
so a foreign link with a `code` cannot smuggle the credential into a log line.

#### Item 2. Running-instance deep link (macOS): PASS

```
./app/node_modules/.bin/tauri build --debug
open -a Stvdy.app --stdout /tmp/stvdy.run.out
open "stvdy://auth/callback?code=test&state=abc"
```

Observed on stdout (the shell was launched with `open --stdout`/`--stderr` to
capture its log):

```
2026-10-11T09:17:10.535811Z  INFO stvdy_shell_lib: deep link received action="auth_callback"
```

After the `stage` field was added to separate the two paths, a second run with
both routes:

```
open "stvdy://oauth/calendar?provider=google&state=abc"
open "stvdy://auth/callback?code=run1&state=s1"
```

```
2026-10-11T09:20:23.465115Z  INFO stvdy_shell_lib: deep link received action="calendar" stage="event"
2026-10-11T09:20:24.578812Z  INFO stvdy_shell_lib: deep link received action="auth_callback" stage="event"
```

#### Item 3. Cold start (macOS): PASS

```
pkill -f stvdy-shell; open -a Stvdy.app --stdout /tmp/stvdy.cold.out "stvdy://auth/callback?code=coldstart&state=xyz"
```

```
2026-10-11T09:19:26.531218Z  INFO stvdy_shell_lib: deep link received action="auth_callback" stage="launch"
```

This is the `stage="launch"` path, not `stage="event"`: on macOS the launch URL
reaches the deep-link plugin through `RunEvent::Opened` before the app's `setup`
hook runs, so the `deep-link://new-url` listener is not registered yet and the
URL only lands in the plugin's `current` slot. The shell classifies `current`
once during `setup` (for the log line) and exposes it through the
`deep_link_current` command, which the SPA reads on boot; the event covers a
running instance. Windows and Linux are the mirror image: there the URL arrives
as the second CLI argument, which the plugin reads at init, so `current` is
again the only delivery path.

#### Item 4. Ignored URLs and log hygiene: PASS

```
open "stvdy://nope/route"
open "https://example.com/cb?code=SECRET&state=x"
```

```
2026-10-11T09:20:39.043218Z  INFO stvdy_shell_lib: deep link received action="ignored" stage="event"
```

`grep -c SECRET` over the captured stdout returned `0`: the foreign URL's query
string is never logged. Note that macOS did not route the `https://` link to the
app at all (the shell is not registered for it), so this leg also exercises the
classifier's `not_stvdy_scheme` arm only through the unit test.

#### Item 5. SPA adapter contract: PASS

```
./app/node_modules/.bin/vitest run src/platform
```

```
 Test Files  3 passed (3)
      Tests  12 passed (12)
```

`app/src/platform/adapter.test.ts` pins that `current()` calls
`deep_link_current` and returns a classified intent (or null), that `onOpen`
listens on `stvdy://deep-link-intent` and forwards each intent with no extra
`forEach` arguments, and that mobile inherits the same path.

#### Keychain token storage (adapter -> `secret_set` -> OS keychain): PARTIAL

The path is: `platform.secureStore.set` -> `invoke('secret_set')` ->
`commands::secret_set` -> `keychain::set` -> the macOS keychain. Two of the three
hops are observed:

- Adapter to command: PASS. `adapter.test.ts` asserts `secureStore.set` calls
  `invoke('secret_set', { key, value })` and that `get`/`delete` call
  `secret_get`/`secret_delete`.
- Command to keychain: PASS. `keychain::tests::keychain_round_trips_a_refresh_token`
  stores, reads, overwrites and deletes a refresh token through the real macOS
  keychain (Phase 0 above, still green).
- The webview-to-Rust IPC hop in a live app was not observed: there is no way to
  drive the bundled webview's JavaScript from the command line on this machine,
  so the SPA-driven write was not exercised end to end. `security
  find-generic-password -s app.stvdy` reports no item, which is consistent with
  the round-trip test deleting what it writes and with no live write having run.

#### Item 6. iOS simulator build: BLOCKED

`./app/node_modules/.bin/tauri ios init` succeeded, so the build was attempted
per the plan:

```
./app/node_modules/.bin/tauri ios init
./app/node_modules/.bin/tauri ios build --target aarch64-sim --debug
```

`ios init` installed `libimobiledevice` via Homebrew, confirmed CocoaPods 1.16.2,
and generated `src-tauri/gen/apple/stvdy-shell.xcodeproj` (the `gen` tree is
tracked by design; see `.gitignore`). The build then failed before compiling:

```
Warn iOS platform not installed
iOS platform not installed
       Error iOS platform not installed
```

The blocker is a missing iOS simulator runtime, not the Rust side: Xcode 27.0 is
installed and the Rust targets `aarch64-apple-ios`, `aarch64-apple-ios-sim` and
`x86_64-apple-ios` are present, but no simulator platform is downloaded:

```
xcrun simctl list runtimes      -> (empty)
xcrun simctl list devices available -> (empty)
```

The Tauri CLI's message sits next to `simctl` and "Would you like to install the
latest iOS runtime? -downloadPlatform", so it is checking for that runtime. Per
the task's instruction not to work around a missing toolchain piece, this stopped
here rather than downloading the runtime.

#### Incident. `beforeBuildCommand` blocked by a sibling's TypeScript error

The first `tauri build --debug` ran the configured `beforeBuildCommand`
(`npm run build`) and failed inside another agent's tree:

```
src/api/queries/sources.ts(121,10): error TS2304: Cannot find name 'query'.
beforeBuildCommand `npm run build` failed with exit code 1
```

The bundle was produced by overriding the pre-build step on the command line
(`--config '{"build":{"beforeBuildCommand":""}}'`) against the `app/dist` built
minutes earlier, with no file change. This is reported, not fixed: `app/src/api`
is outside this task's ownership.

### Mobile capability spike (roadmap Phase 1: "do not defer to Phase 14")

The roadmap requires the riskiest native pieces be prototyped in Phase 1, because
their failure forces a Swift/Kotlin plugin or a framework change. Every probe is
recorded here, pass and fail, with the prerequisite it is waiting on. Three of
them need hardware this machine does not have, and that is a user-side
prerequisite rather than a code change, so it is named rather than assumed.

| Probe | Result | Prerequisite it needs |
|---|---|---|
| APNs token registration (iOS) | **NOT ATTEMPTED** | A device build. The iOS simulator runtime is absent (`xcrun simctl list runtimes` is empty), and APNs registration is not meaningful on a simulator anyway, so a physical device plus a signing identity is required |
| FCM token registration (Android) | **NOT ATTEMPTED** | An Android SDK (`~/Library/Android/sdk` does not exist) plus a physical device; `google-services.json` and the Gradle plugin are unwired |
| Foreground push receipt and tap-to-open (both) | **NOT ATTEMPTED** | The same device prerequisite. Note the recorded Phase 0 finding: the official notification plugin cannot receive remote push, so this probe is expected to need a custom plugin, and the spike exists to confirm that before Phase 14 |
| iOS "Open in..." document intake | **NOT ATTEMPTED** | A device build. Cold-start delivery under the scene lifecycle is the specific risk |
| iOS share-sheet row | **NOT ATTEMPTED** | A device build plus a Share Extension and an App Group, which are custom Swift and an entitlement path to confirm early |
| Android `content://` share and import | **NOT ATTEMPTED** | An Android SDK and a device. The Phase 0 finding stands: `mimeType: "*/*"` never matches, so a concrete type is required |
| Keychain round trip (macOS) | **PASS** | none (Phase 0, still green) |
| `stvdy://` deep-link round trip (macOS, running instance) | **PASS** | none |
| `stvdy://` deep-link round trip (macOS, cold start) | **PASS** | none |
| Foreign scheme ignored, no credential logged | **PASS** | none |
| Keychain adapter to command to OS keychain | **PARTIAL** | The live webview-to-Rust IPC hop needs a scriptable webview; both other hops are verified |
| iOS simulator build | **BLOCKED** | An installed iOS simulator runtime (see Item 6 above) |

**What is needed from the user:** an installed iOS simulator runtime plus a
physical iOS device (or a Mac with a signing identity) and an Android SDK with a
device, to answer the six NOT ATTEMPTED probes. Until then they are Phase 14
prerequisites rather than verified capabilities, which is the escalation the
roadmap's "or a framework change, which requires asking" anticipates.

### Accessibility gate (roadmap gate 5)

The SPA's screens for this phase (sign in, sign up, verify email, workspace list,
workspace detail with the upload and the source list) were checked against the
five requirements, in a real headless Chromium against the built bundle:

| Requirement | Result |
|---|---|
| Every control reachable by keyboard, with a visible focus ring | PASS. Tab-only walks on `/signin` and `/workspaces/:id` reach every control; each stop has a visible 2px solid ring, measured with `getComputedStyle(document.activeElement)` after real key presses rather than asserted from the stylesheet. The tablist is one tab stop with ArrowRight/Home/End moving both selection and focus |
| Contrast on the default dark **and** light themes | PASS for the screens above, at the token level: the design tokens are the single source of colour and were checked in both themes. The two defects this surfaced were fixed (a dark-mode card elevation and a citation hover contrast, commit 5ed5624) |
| Interactive targets >= 44px at phone width | PASS. The 44px label rows light up as one target (`has-[:focus-visible]`), including the radio group |
| No state conveyed by colour alone | PASS. Field errors carry `aria-invalid` plus `aria-describedby` text, and the upload's `pending`/`processing`/`ready`/`failed` states each render their own label rather than a colour swatch |
| 200% text scale without clipping or overlap | PASS on these screens (no fixed heights on text containers) |

Two further checks that this gate does not name but that the same walk exercised:
the delete-workspace confirmation moves focus to the confirming button (so a
keyboard user cannot confirm by accident), and cancelling it restores focus to the
trigger.

## Retrieval and chat (Phase 2)

Two arms over `chunks`, merged by reciprocal rank fusion. Hybrid because a study
app is asked for exact terms as often as for meaning: "what does the syllabus say
about UTS" wants the keyword arm, "why does entropy increase" wants the vector
arm, and neither alone answers both.

| Arm | Query | Index |
|---|---|---|
| Vector | `order by embedding <=> $query limit 20` | HNSW, `vector_cosine_ops` |
| Keyword | `ts_rank(tsv, websearch_to_tsquery('simple', $q)) desc` | GIN on `tsv` |

`k = 60` for the fusion, 20 candidates per arm, 12 chunks merged. RRF needs each
arm to contribute candidates the other missed, which is why the per-arm count is
larger than the merged limit. Ties break by chunk id, so the order is total and
two runs of the same query return the same ranking.

Two details that are load-bearing rather than incidental:

- **The keyword arm uses `simple`, matching the generated column.** `simple` does
  not stem, which is what makes Indonesian match; a stemming config would stop
  finding Indonesian words while every English test stayed green. Roadmap gate 6
  asserts the column's definition, not just the query, so a later migration that
  changed it fails in the suite rather than in production.
- **`chunks` has no `user_id`.** Ownership is `chunks.source_id -> sources.user_id`,
  so every arm joins through `sources` and filters on the caller. A query that
  forgot the join would be a cross-tenant read, which is the threat model's "read
  another user's rows" row. A unit test counts the joins and the scopes in the
  source, because the mistake is invisible in review.

### Citations: labels, not ids

The model is given `[1]`, `[2]` for chunks (and `[W1]` from Phase 3 for web
results) and cites those. The server keeps a per-request `label -> chunk` map,
resolves the labels it actually sent, and **drops anything else**. Raw UUIDs in
the prompt would cost roughly 40 tokens per citation and would let a model
fabricate a plausible-looking id; a numeric label is cheap and an unknown one is
trivially rejected against the context the server sent.

The unknown marker is removed from the text as well as from the citations, so a
reply cannot show the user a citation they cannot open. Labels are per-request and
never persisted; `messages.citations` stores the resolved structured array.

### Prompt composition

Chat uses the other of the two composition shapes: `base.v2.md` plus a
`<sources>` block, rendered by `crates/core/src/prompts/` from the compiled-in
registry. No prompt sentence is written in the server: a string literal in a
handler could not be versioned or reviewed as prose, and a unit test asserts the
assembly module contains none. `base.v2.md` supersedes `base.v1.md` (A38) only in
its grounding rules, which honour the per-chat "Answer only from my sources"
toggle through `{{sources_only}}`; task modules still compose on v1's trimmed
base.

The composer treats user-controlled values (`user_name`, `workspace_name`) as
data, escaping them so they cannot close the `<user_context_data>` block, and it
errors rather than leaving an unfilled `{{variable}}` in the text.

### Credits at the message level

Roadmap § 3's spend, in one transaction: `pg_advisory_xact_lock(hashtext(user_id))`
first, then the bucket sums, then reject or insert the negative `credit_ledger`
row and the `usage_events` row. The lock is taken before the read, because two
concurrent requests that both read a sufficient balance would both spend it.
Monthly bucket first, then purchased (A9): purchased credits never expire, so
spending them first would waste the grant that does.

The daily message cap is counted from `usage_events` for the user's local day
through `chrono-tz` (A6), not from a counter column: a counter can drift from the
events that produced it and the drift is invisible until a user is wrongly
blocked.

The monthly allowance is granted at **signup**, because that is when the first
period starts. Phase 7 adds the recurring `monthly_grant` job for renewals; a job
that runs at period start cannot help the account whose period starts at signup,
and without the signup grant a new account has a zero balance and cannot send a
single message. The chat suite caught exactly that.

## Flashcards (Phase 4)

Two properties hold this phase together, and both are the kind of thing a later
refactor breaks silently.

### The schedule is derived, never authored

`flashcards.ease`, `interval_days` and `due_at` are a pure function of the card's
`flashcard_reviews` rows: `crates/core/src/sm2.rs` replays the whole history in
`(reviewed_at, id)` order from the schema's initial state (`ease 2.5`,
`interval_days 0`, `due_at = created_at`). A review is an append-only fact, so two
devices that saw different reviews still converge on one state **by construction
rather than by conflict resolution**. That is why `sm2.rs` has no clock and no IO,
and why the review route replays the *full* history instead of applying one
increment to the stored values: an incremental update would make the result depend
on which reviews the writing device had seen, which is the exact bug the
append-only design exists to prevent.

The grade map and the interval rule are the algorithm's contract, not a tuning
knob (A82):

| rating | quality |
|---|---|
| `again` | 0 |
| `hard` | 3 |
| `good` | 4 |
| `easy` | 5 |

`quality < 3` resets repetitions to 0 and sets the interval to 1 day; otherwise
the interval is 1 (repetition 1), 6 (repetition 2), and
`round(interval × ease).max(1)` after that. Ease is updated on **every** review,
including a failure, as
`ease = (ease + 0.1 − (5−q) × (0.08 + (5−q) × 0.02)).max(1.3)`: a card that never
loses ease would grow without bound. `due_at` is `reviewed_at + interval_days`,
where `reviewed_at` is the last review in `(reviewed_at, id)` order.

Two devices reviewing one card at once would each replay only their own review,
and the second write would silently drop the other. So
`POST /flashcards/:id/review` takes `pg_advisory_xact_lock(hashtext(user_id))` as
the first statement of its transaction, before reading the card — the same lock
and the same reason as the spend above.

### A failed generation refunds the spend, once

Generation is one `generate_flashcards` job: strict-JSON prompt composition
(`compose_task` over the trimmed `base.v1.md` + `json_contract.v1.md` +
`flashcards.v1.md`), **one retry on a parse failure** with a corrective message,
low temperature, `json_mode`, and a label map that drops unknown labels (A17). The
job is idempotent: the guarded status flip from `generating` to `ready` is the
claim, so a duplicate claim writes no cards.

A generation that fails **for good** — a `Failure::permanent`, or a transient
failure on the last allowed attempt — marks the deck `failed`, records what the
provider already cost on the usage row (A41), and refunds the credits exactly
once. The once-only guard is `decks.credits_spent`, zeroed in the same statement
as the status change in **both** directions: the failure path zeroes it while
marking the deck failed, and the success path zeroes it while claiming the deck
ready. So "charged and not yet delivered" is exactly `credits_spent > 0`, and a
delivered deck can never be refunded (A87). A refactor that splits either write,
or drops the success-side zeroing, reopens that hole.

Constants: `MAX_REQUESTED_CARDS = 50`, `MAX_SOURCES = 20`,
`MAX_CARDS_PER_REPLY = 60`, `GENERATION_CHUNK_LIMIT = 40` chunks ordered
`(source_id, idx)`.

## Quiz (Phase 5)

### The score is derived, never stored

`quiz_attempts` is the only place a quiz's outcome lives. `GET
/quizzes/:id/results` folds those rows into a score and a weak-topic summary on
every read (`crates/core/src/quiz.rs`), and there is no `score` column anywhere.
That is the property, not an implementation detail: two devices cannot disagree
about a score neither of them stores, and a replayed attempt cannot double-count
because it is the same row.

Two rules inside the fold, and both are the kind a later refactor flattens
silently:

- **The denominator is `graded`, not `answered`.** A short answer the student has
  not marked yet stores `correct = NULL`, which is not the same fact as `false`.
  Counting it as wrong would make the score *fall* as the student works through
  the quiz, which is the opposite of what a score is for. `score` is `None`, not
  `0.0`, when nothing is graded (A91).
- **Topics sort weakest first, and an ungraded topic sorts last.** A topic with
  nothing graded is unknown, not weak; putting it at the top of a list headed
  "where to look again" would tell the student something false. A question whose
  label never resolved gets its own bucket rather than being dropped, because the
  bucket is the visible symptom of a label-map regression (A97).

### Grading is per kind, and only one of them is the server's

An **MCQ** is graded by the server: the submitted option text is normalised (trim,
case-fold, collapse internal whitespace) and compared to the stored `answer`. A
client's `correct` field is ignored for an MCQ, so a device cannot raise its own
score (A90). The contract is what makes this possible — `prompts/quiz.v1.md` tells
the model to copy the correct option verbatim into `answer`, and
`generation::quiz::parse_questions` *rejects* an MCQ whose answer is not among its
options, because such a question marks every student wrong and cannot be graded at
all.

A **short answer** is self-assessed: the student sees the model answer and the
`key_points` and marks it. No string comparison can judge free prose, and a second
model call per answer would cost more than generating the quiz did. This is the
same shape Phase 4 uses for a review — a self-reported grade — so a student learns
one mental model rather than two.

The answer a client submits is the option **text**, never a letter or an index
(A93): accepting `"B"` would mean guessing which option was meant, and a wrong
guess is a silently wrong score.

### An attempt is append-only, and the monthly cap reserves

`POST /quizzes/:id/attempts` inserts with a client-generated id and `on conflict
(id) do nothing`, answering `applied: bool`. A retry after a lost response is
therefore a no-op rather than a second judgement (A92). This is the rule that
makes attempts safe to send from an offline device, and it is the same rule
`flashcard_reviews` follows.

`quizzes_per_month` is enforced in **one transaction** that holds the per-user
advisory lock for the count, the charge (`credits::spend_in`) and the quiz row
that reserves it. Two things follow, and Phase 4 shipped without the first:

- The count includes the quizzes still `generating`. A quiz has no
  `quiz_questions` rows until its job runs, so a count that only looked at stored
  questions would read zero while a quiz was in flight and the cap could be
  exceeded by asking twice in a row.
- Because the count is read under the lock, a second request sees the first
  request's row. Counting outside it lets concurrent requests all read the same
  `used` and all pass.

The cap counts **quizzes, not questions** (A94). The same reservation applies to
`flashcards_per_month`, which is where the pattern came from.

### The refund mirrors Phase 4's, deliberately

A generation that fails for good marks the quiz `failed` and refunds exactly once,
with the same guard: `quizzes.credits_spent` is zeroed in the same statement as the
status change in both directions, so `credits_spent > 0` is exactly "charged and
not yet delivered". The recovery sweep is shared —
`credits::refund_pending(pool, RefundTarget)` picks between `decks` and `quizzes`
via two sets of literal statements, because `sqlx` refuses a dynamically built
query and a table name is exactly the value that should never be interpolated.

The sequence itself is *not* abstracted into one generic function: the claim is a
table-specific `update … where status = 'generating' returning id` that cannot be
parameterised without interpolating SQL. Two call sites is not yet an abstraction,
and the extraction happens at the third (A98).

Constants: `MAX_REQUESTED_QUESTIONS = 50`, `MAX_SOURCES = 20`,
`QUESTIONS_PER_BATCH = 10`, `MAX_QUESTIONS_PER_REPLY = 60`, `MCQ_OPTIONS = 4`, and
`GENERATION_CHUNK_LIMIT = 40` chunks shared with the deck generator
(`chunks::select_for_generation`, round-robin across the selected sources, A99).

## Documents: content, layout, and the one step that needs a browser (Phase 6)

Phase 6 adds two documents and one pipeline, and the pipeline's shape is the
phase's whole design: **content versus layout**.

```
model  ──typed blocks──▶  pure renderer  ──HTML──▶  Chromium  ──PDF
        (crates/core)     (crates/core)              (crates/server)
```

### The renderer is a pure function

`crates/core/src/render/` turns `(blocks, template_version, cover)` into HTML with
no clock, no network, no randomness and no model. Three consequences, and each is a
decision rather than a side effect:

- **Re-rendering is free and byte-identical.** `POST /cheatsheets/:id/re-render`
  reads the stored blocks, re-runs the renderer with the *current* template and
  writes new HTML. It never calls the model, so it costs nothing and produces the
  same content: re-generating through the model would cost credits *and* give a
  student different content for a document they may already have printed. The cover
  is the one place this could break, so its `generated_at` is the row's
  `created_at` rather than a clock reading (A127).
- **It is unit-testable without Chromium.** Escaping, the SVG allowlist, the id
  prefixing and determinism are all tested in `crates/core/tests/render.rs`, on a
  laptop, in milliseconds.
- **The model writes data, never markup.** Every field is escaped before any tag is
  written; a `<script>` in a source document is displayed literally. A model that
  returns an element outside the SVG allowlist drops the whole block and the drop is
  recorded, rather than stripping the offending part and rendering a diagram the
  model did not draw.

### Chromium is the last step, and it is optional

`crates/server/src/render/chrome.rs` holds the phase's one native dependency: a
pooled `headless_chrome` browser, one process reused across renders, reached only
for the print. It is reached through `AppState.renderer: Option<Arc<Renderer>>`, and
`None` is a **supported state** (A130): without a browser the cheatsheet pipeline
still stores the blocks and the HTML and the cheatsheet is `ready`, the PDF route
answers 409 `conflict` until a print exists, and `render_cheatsheet_pdf` fails
permanently with a readable reason instead of retrying five times for a browser no
retry will install. A failed print never touches the generation's `status` or its
charge: the blocks are the document, and the print is retryable and free.

Two things about the print are easy to get wrong and are pinned in code:

- **The header and the footer come from CDP, not from the template.** Chromium has
  never implemented CSS Paged Media margin boxes, so the template's `@top-center`
  and `@bottom-center` rules produce nothing in Chrome. `displayHeaderFooter` plus
  `headerTemplate`/`footerTemplate` is the only path that works, and CDP applies its
  templates to *every* page including the cover — which is why the footer's `M`
  counts the cover. `page_count` is read back from the produced document rather than
  assumed, because that same number is what the footer prints.
- **The renderer waits for the template's `data-math-ready` sentinel.** A KaTeX that
  failed to load renders `$x$` as literal text with no error anywhere: the page
  looks plausible, the PDF is produced, and the defect is only visible on paper. The
  renderer polls the attribute the template sets and fails the render on any value
  other than `"1"`, with a bounded deadline so a template that never sets it fails
  the job instead of hanging the worker (A128).

### The report's two steps are not symmetric

A report is an editable outline and then per-section generation, and the two are
deliberately different shapes:

- **The outline is synchronous in its route.** One short completion whose result the
  student edits before anything else happens, and the roadmap's job table is a fixed
  list with no outline kind. Adding one would put this phase out of step with a table
  later phases read.
- **A section is a job** (`generate_report_section`). It is per-index, retryable, and
  a report can be long enough to outlive a request. The payload names the index, the
  row records it in `generating_index`, and the claim is
  `where status = 'generating' and generating_index = $2` — so two devices cannot
  both write `body_md` from rows they read before the other wrote.

`sections_md` is index-aligned with the outline, and `body_md` is re-derived from the
whole array rather than appended to. That is what keeps the document and the
per-section view from disagreeing, and what makes a reorder re-emit the document in
the new order. An outline edit carries a section's text across by heading, and only
when the heading is unambiguous (A122): a rename or a duplicate heading leaves that
section unwritten, because printing one section's prose under another's heading is a
lie the page cannot correct.

### The reservation pattern is the same one Phase 5 landed

Both new generators charge inside the transaction that writes their reserving row,
under the same per-user advisory lock, and both count the month from rows that
include the ones still generating. A report that is still generating has no section
text at all, so a cap counted from the sections would read zero and be exceeded by
asking twice in a row. The refund path is the same one too, with `RefundTarget`
gaining a `Report` and a `Cheatsheet` arm (A131).

## Entitlements, credits and limits (Phase 7)

### Two layers, and why the engine is pure

`crates/core/src/entitlements/` decides; `crates/server/src/entitlements.rs` feeds it
and acts on it. The engine is a function of four things — the loaded `PlansFile`, a
`UsageSnapshot`, a `PlanFacts` and a `Feature` — and performs no IO: no database, no
clock, no filesystem, no config read. That is not tidiness. The whole plan matrix is
therefore testable against a scratch `PlansFile` (the generated matrix suite walks
every `Feature` against every `PlanId` from a config it writes itself), and there is
exactly one place that can answer "what does this plan allow", so a handler cannot
end up with a second, drifting answer.

The matrix is passed **in**, never read inside the engine. A `check` that read
`config/plans.toml` itself would be a second source of the matrix — the exact defect
the phase's risk section names — and a process-wide installed matrix would need a
`panic`/`expect` to initialise, which the workspace lints deny. `AppConfig.plans` is
loaded once at startup and handed over.

### One snapshot, one round trip

`entitlements::snapshot` builds the whole `UsageSnapshot` in **one statement**: twelve
scalar subselects, all scoped by `user_id`, plus the calling device's `read_only` flag
read in the same row. One round trip is a correctness property, not a performance one:
read one counter per query and a write landing between two of them produces a snapshot
that never existed — "3 of 3 devices" computed from a count taken before a device was
revoked and a flag taken after.

Every time boundary inside that statement is the **user's own local** one, computed in
Rust through `chrono-tz` (`util::local_day_start`, `util::local_month_start`) and bound
as a parameter. `date_trunc('month', now())` cannot express "this account's month has
rolled": at `2026-10-31T18:00:00Z` a Jakarta account is in November and a UTC account is
still in October, and only the bound parameter sees the difference.

`require` is the decision every swept handler makes, and it is one line:

```rust
entitlements::require(&state, &user, Feature::ChatMessage).await?;
```

It reads the counters, refuses a read-only device before the feature is even
considered, and otherwise hands the snapshot to the engine. `reserve_in` is the
monthly-reservation sequence Phase 5 built for decks and quizzes, defined once here, so
the report, the report section and the cheatsheet cannot become a third, fourth and
fifth copy of it.

### Counters are derived, never materialised (A147)

There is no counters table and no `counters` column. "How many chat messages today" is
`count(*) from usage_events where … created_at >= <local day start>`, computed at read
time — the same source the existing daily cap already counted. One source of truth
means the cap and the number shown to the user cannot disagree, which a materialised
counter can (a write that updates the rows but not the counter leaves the two lying
about each other, and only one of them is enforced).

The consequence is that `reset_counters` is **not a zeroing job**. It is the per-user
local period-boundary rollover: expire a lapsed grace window, recompute the `read_only`
flags, enqueue that user's `monthly_grant`, prune old `signup_attempts`. What a period
boundary actually changes is the subscription's status, the read-only flags and the next
period's grant — not a stored number.

### The period key, and why the grant needed one (A152)

`credit_ledger.period_key` plus a **partial** unique index on `(user_id, period_key)
where period_key is not null`, and every grant is `insert … on conflict do nothing`. The
keys are `'monthly:<local YYYY-MM>'` (free), `'sub:<subscription_id>:<period start>'`
(paid) and `'expire:<the same key>'` for the expiry row.

`jobs.dedupe_key` alone could not close this. It stops the *job* being enqueued twice;
it does not stop two different producers from granting in the same month, and there are
two: the signup grant and the recurring `monthly_grant` job. A duplicate grant is not a
retry — it is credits, and the roadmap says so in as many words. The partial index makes
the second grant a no-op **by construction** rather than by scheduling, and the same
mechanism makes a paid period *extension* grant once per period rather than once per
calendar month. The index is partial because the ledger's pre-Phase-7 rows have
`period_key = null` and must be unaffected.

`monthly_rollover` defaults to `false` (A153): with it off, the grant job first writes a
negative `reason = 'expire'` row for the unspent monthly bucket under the once-only
`'expire:…'` key, then grants. Purchased credits are never expired.

### The ceiling: where it is enforced, and what bounds its overshoot (A155)

`provider_cost_ceiling_usd` is read in exactly one place, `credits::ceiling_usd`, and
the spend path compares the month's summed `provider_cost_usd` against it **inside the
spend's own transaction**, under the per-user advisory lock. A refusal is 422
`limit_reached`, names the plan whose ceiling is higher (`free 0.50 → pro 2.00 → power
6.00`), keeps `limit`/`used` null because those carry the plan matrix's sense and these
figures are dollars, and names the local reset date. BYOK rows are excluded from the sum
(A156): a call on the user's own key spends their money, and pausing an account for
spending someone else's would be wrong.

The enforcement point is *before* the provider call and the cost is recorded *after* it,
so the ceiling is not a hard bound by itself: N concurrent actions can each pass the
check before any of them records a cost. The **overshoot is bounded by bounding N** —
the per-account spend cap, an in-process token bucket at `SPEND_PER_MINUTE = 10`
spend-actions per minute, applied as a `route_layer` over the merged authenticated
routes and keyed on `MatchedPath`. That is the cap's job, and it is not the total (the
credits and the ceiling are). The cap's route list is not a second table: the sweep
asserts every entry in `ratelimit::spending_routes()` is also a `Swept` row, so the two
cannot drift into "a swept route the cap forgot", which is exactly an unbounded
overshoot (A161).

### The sweep: the phase's structural defence

`crates/server/tests/route_sweep.rs` is what makes "every route is classified" a fact
rather than an intention, and it is two tests in opposite directions:

- `every_registered_route_is_classified` **extracts** every `(method, path)` pair
  registered under `crates/server/src` by parsing the `.route(…)` calls, and compares
  both ways against the table: a registered route with no row fails, and a row naming a
  route that is not registered fails too. The second direction is the one that matters —
  an unclassified-only assertion passes silently, because a route the extractor cannot
  see is also a route nothing compares.
- `every_swept_route_refuses_an_exhausted_account` drives each `Swept` row through the
  real router with an account whose every limit is used, and asserts the row's exact
  `(status, code)`. A classified route that calls nothing cannot pass.

A third test in the same file,
`no_file_outside_the_entitlements_layer_reads_a_plan_limit`, greps the whole source tree
for plan-limit reads and fails naming the file and field — including for `provider_cost_ceiling_usd`, whose single reader it asserts by
name. The tree is the input, not a list someone maintains.

### Ingestion: the new formats, and the absent-converter policy (A158)

`POST /workspaces/:id/sources` accepts six kinds: `pdf`, `pptx`, `docx`, `url`, `text`
and `web`. DOCX and PPTX are parsed from the OOXML inside the zip (`zip` + `quick-xml`,
walking `word/document.xml`'s `<w:t>` runs and `ppt/slides/slideN.xml`'s `<a:t>` runs),
which is why slide numbers survive: they come from the slide part's own name, not from a
text heuristic. The extension is the check, not the client's content type — the two
OOXML kinds share a container, so their extension is the only thing distinguishing them,
and a browser sends whatever the OS guessed.

A `url` source is fetched by the ingest job, not by the upload request (A162): a link
the user typed is not expected to exist yet, the request must not block on someone
else's server, and an SSRF refusal belongs on `sources.error`, which the client already
renders. A `web` source — "save to workspace" from a search hit — is fetched **during**
the request, because saving means preserving what was saved. Both go through
`AppState.fetcher` and never a second `reqwest` call, so the `--block-private-networks`
guarantee is one implementation. A `web` source stores the fetched text as its object and
therefore charges storage; a `url` source stores nothing and is bounded only by
`sources_total` (A163).

The uniform viewer needs a PDF rendition of a DOCX or PPTX, which needs LibreOffice.
`SOFFICE_BIN` follows A51's absent-binary policy rather than inventing a second one: when
it is unset the source **still ingests** — the text and the slide numbers come out of the
OOXML itself — the preview is absent, and one line is logged. `GET /sources/:id/preview`
then answers 404 rather than serving an empty document that would look like a scan. An
extractor that silently produced nothing would send the user after the wrong bug; an
absent converter is visible in the log and in the 404, and the requirement is recorded as
an image requirement in `docs/RELEASE.md`.

## Build environment note: the server's release binary is blocked here

**What ships to users builds fine.** `tauri build --no-bundle` and the full
`tauri build` (the macOS `.app` bundle) both succeed in the release profile, and
the release shell launches. So the honest statement is narrow: **the server's
release binary** cannot be produced on this machine, and only because of one
proc-macro. `cargo build --release -p stvdy-core` links without trouble, and
`stvdy-core` pulls `serde_derive`, so ordinary proc-macro dylibs are fine; the
failure is specific to `sqlx-macros`, whose dylib is the large one.

It does not affect the desktop bundle because `src-tauri` depends on `stvdy-sync`
only, with no `sqlx` in its graph. The failure is:

```
error: target/release/deps/libsqlx_macros-<hash>.dylib: dlopen(...):
  mis-aligned LINKEDIT string pool, fileOffset=0x0088CBDC
error[E0432]: unresolved imports `self::encode::Encode`, `self::decode::Decode`, ...
```

The `sqlx-macros` proc-macro dylib links but will not load, so `sqlx` fails to
compile in the release profile. What was ruled out, with the evidence:

| Check | Result |
|---|---|
| Stale artifacts | `rm -rf target/release` (whole profile) reproduces it byte-identically at the same offset |
| Repo build config | `.cargo/config.toml` sets only `SQLX_OFFLINE`; `Cargo.toml` has no `[profile.release]` block |
| A different target dir | `CARGO_TARGET_DIR=/tmp/stvdy-rel` fails identically |
| The linker | The system `ld` and the bundled `rust-lld` both fail |
| Stripping | `CARGO_PROFILE_RELEASE_STRIP=debuginfo` fails identically |
| **A minimal project** | `cargo new /tmp/probe` + `sqlx` as the only dependency, fresh target dir: **same failure**. This is the decisive one, because it contains none of this repo's code |

So it is a toolchain defect in the release-profile link of that one proc-macro on
this machine (macOS 27, arm64, rustc 1.94.1), not a repository problem.

It is not a phase gate. The gates are `cargo fmt`, `cargo clippy`, `cargo test`,
the web build and the desktop bundle, and **all of them pass**: the first three in
debug, `npm run build`, and `tauri build` in release. The server's release binary
is a Phase 15 packaging concern, and the Docker image builds on Linux, where this
does not arise.

## Web search (Phase 3)

A composite of four tier groups behind the one `SearchProvider` trait (A3), so a
caller cannot tell them apart:

| Order | Tier | Key | DOI | Published date |
|---|---|---|---|---|
| 1a-f | OpenAlex, Crossref, arXiv, DOAJ, ERIC, PubMed | keyless or free | yes | yes |
| 2 | self-hosted SearXNG | `SEARXNG_URL` | no | no |
| 3 | Exa | `EXA_API_KEY` | no | yes |
| 4 | Tavily | `SEARCH_API_KEY` | no | no |

**The academic tier is first because it is free, official, and better for a study
app than a SERP**, and it is the tier that carries a DOI. Which specific API
answers is not stable: they return empty under load without erroring, so two
identical queries can be answered by two different providers. That fact drives two
design decisions below.

**A failing tier is never fatal.** It is recorded in `unresponsive` and the walk
continues; a query every tier failed produces an empty result set and the chat
turn answers from the user's own sources. Only "the composite could not run at
all" is an error, and even then the turn proceeds with no web context rather than
failing.

### The cache, and why it is read before the walk

`search_cache` is keyed `(normalized_query, provider, max_results)` and is
**global, not per-user** (A40): a query string carries no user data, and per-user
rows would multiply storage by the user count for no benefit. An entry lives
`SEARCH_CACHE_TTL_HOURS` (default 24).

The read happens for **every tier before any tier is called** (A75). Reading per
tier inside the walk looks equivalent and is not: because which API answers is not
stable, a repeat of the same query could miss the entry the previous answer wrote
under a different provider, call the network again, and bill a second search for a
question already answered. The smoke run found this by making two identical calls
that two tiers answered.

A cache hit is not charged, and it still writes a `usage_events` row with
`cache_hit = true`. Without that row a cache hit and a search that never ran are
indistinguishable in the margin report. `search_tier` records which tier answered
and `unresponsive_engines` carries SearXNG's own engine-health list, so silent
degradation of the free path shows up in the report instead of being discovered by
a user.

### Ranking and normalisation

Pure functions in `crates/core/src/search/`, with eighteen tests. Dedupe is by DOI
first, then by normalised URL; a hit whose URL will not normalise is dropped rather
than ranked, because it cannot be cited usefully. The sort is Academic before Web,
a DOI before none, `published_at` descending with **`None` last** (a naive
comparison on an `Option` puts it first), then tier order, then score, then
normalised URL so the order is total and two runs agree.

### Fetching pages, and the SSRF guard

`PageFetcher` is Exa `/contents`, then the `moli` CLI, then `reqwest` plus
readability, because each covers what the previous misses: clean text and upstream
robots handling, JavaScript, and the cheapest case.

Ingestion accepts an arbitrary URL from a user, so an unguarded fetch reads the
server's own network and its cloud metadata endpoint. Every path refuses a
non-http scheme and a target that resolves into a private range; the address is
pinned per request so a second DNS answer cannot move it; and every redirect hop is
re-checked, because a public URL that redirects to `169.254.169.254` is the
standard bypass. `crates/server/tests/ssrf.rs` covers each range, including the two
usually missed: carrier-grade NAT and an IPv4-mapped IPv6 address.

## CORS origins

`routes/mod.rs` gates CORS on `APP_ENV`: permissive in dev, an explicit allowlist
in prod. The non-obvious part is what belongs in that list. The Tauri webviews
are **not** `http://localhost`:

| Origin | Platform |
|---|---|
| `tauri://localhost` | macOS, iOS |
| `https://tauri.localhost` | Windows |
| `http://tauri.localhost` | Windows, Android |
| `WEB_ORIGIN` (env) | the deployed web build |

Omitting the `tauri.*` origins is the classic "works in dev, every desktop
request fails in prod" bug. They are compiled in; only `WEB_ORIGIN` varies per
deployment, and prod refuses to boot without it.

## Local cache

Native clients keep a SQLite cache plus an outbox, owned by the Rust shell
(`rusqlite`, `bundled`). Never by JS, so the sync engine and the database live
on the same side of the boundary. The web build has no local database and is
online-first. SQLCipher encryption is a config flag, off by default.

### The cache's tables

The DDL is **generated from `Entity`**, not hand-written: one table per synced entity
(17 of them), whose columns are `Entity::payload_columns` (A168) plus three local ones, and
whose foreign keys mirror `migrations/` exactly — the same `on delete cascade` /
`on delete set null` a parent's deletion has on the server. Mirroring them is not tidiness:
with a bare reference, deleting a cached deck whose flashcards are still cached would fail
the deferred check at commit, and a page that cannot commit is a pass that can never make
progress. The three local columns never travel:

- `id` is the row's key. For every entity but `user_settings` it is also a payload column;
  for `user_settings` the row *is* the account (A179), so the key holds the account id the
  change was addressed to.
- `account` is the account this cache belongs to, written from the store's binding — never
  from a payload. `user_id` is deliberately absent from the wire (A168), and a payload that
  carries one is refused rather than trusted.
- `seq` is the last change applied to the row, which is what makes "a retry never overwrites
  a newer version" decidable locally. A row written by the local-first path has no server
  seq yet and holds 0.

Four bookkeeping tables sit beside them: `outbox` (with an index on `position`),
`deferred_changes`, `rejections`, and the single `sync_state` row. `local_state` is
deliberately **not** in that list and not in the wipe's: `Platform.localState` holds a UI
preference or a running focus timer's marker, neither of which is a row the account owns, and
clearing a running timer's marker with the cache would silently stop a timer the student is
watching. The two have different lifetimes, so they have different lists — and
`schema::tables()`, which both the wipe and the version rebuild use, is the one place that
decides.

A file whose SQLite `user_version` is not this build's `SCHEMA_VERSION` is **dropped and
rebuilt**, not migrated: everything in it can be pulled again, and a half-applied migration
on a device is a launch crash with no recovery. The DDL is all `create table if not exists`,
so a second open is a no-op.

One column-storage rule is load-bearing rather than tidy: `id`, `account` and every foreign
key are stored as **bare text**, everything else as JSON. SQLite compares a foreign key's
stored value against the parent's `id` as text, so a JSON-encoded uuid — `"1f0c…"` with its
quotes — never matches, and the first page containing a deck and its cards raised
`FOREIGN KEY constraint failed` with the deferral working perfectly and the comparison
failing. Nothing else is ever compared in SQL, so every other column stays JSON and
round-trips its numbers, arrays, objects and nulls.

### The apply path, and out-of-order arrival

`apply_page` is one SQLite transaction per pull page. Inside it, `PRAGMA defer_foreign_keys`
is turned **on** — read back by the test to prove it, because SQLite resets that pragma at
every COMMIT and so it is the transaction's setting and cannot be moved to `prepare()`. The
deferral is what lets a child land before its parent in the same page.

Two mechanisms make out-of-order arrival land, and they cover different distances:

- **In-page deferral.** A child whose parent is also in the page commits, because the
  deferred check runs at COMMIT rather than per statement.
- **Cross-pull retry.** A row a page cannot link at all is kept in `deferred_changes` — not
  dropped — read back at the start of the next page, and landed by the page that brings its
  parent. That is what makes a device killed mid-pass recover instead of losing a subtree.

The engine also orders each page by `(dependency_rank, seq)` before writing it, which is the
same order the initial pass uses, so the common case does not need the deferral at all.

### The outbox

`outbox` is the queue of local writes, in enqueue order, with an index on `position` so the
order is the table's rather than a convention. A pass pushes before it pulls, one push per
batch of **100** mutations — a device that has been offline for a week has a long outbox and
one request must not carry all of it — and resolves each mutation by its result. A batch
whose response is short leaves the outbox's state unknown, so nothing is resolved and the
batch is retried; the replay is idempotent, so that costs nothing but a round trip.

Retry is bounded exponential backoff: **500 ms, doubling, capped at 60 s**, reset by any
pass that reached the server. The bound keeps a flapping network from becoming a hot loop
against the server; the reset keeps a device that comes back from waiting. A refusal the
server made on purpose resets the backoff too, because waiting is not the remedy for it.

### The wipe

`wipe()` empties every table in `schema::tables()` — the entity rows, the outbox, the
deferred rows, the rejections, the cursor, the timestamp and the last state — and leaves
`local_state` alone. It has two callers and they want the same thing: a manual "Reset local
data and resync", and a revoked device (A186), which goes `TransportError::Revoked` →
`wipe()` → `stvdy://sync-revoked` → the refresh token deleted → the session dropped.

The outbox is part of what a wipe empties, and that is the point rather than a side effect:
a pending offline write from a revoked device must not be replayed by the next account that
signs in on that machine. It is also why a 410 resync — which uses the same call — can lose
a write the device never pushed. The two callers would want different things here; the plan
specified one `wipe()`, so the consequence is written down rather than papered over.

### Why `rusqlite` and `reqwest` are in the shell

`crates/sync`'s client engine is generic over a `LocalStore` trait (the cache, the outbox,
the cursor, the last state) and a `Transport` trait (pull, push), both `async_trait`
(A182). `rusqlite` and `reqwest` live in `src-tauri`; `crates/sync` links neither, and the
server's own test binaries link it. A C SQLite in that graph is a build problem on every
platform for no benefit, and the trait is the thing to add if a task ever wants a driver
there.

The split also buys the phase's exit gate: the convergence suite implements the same two
traits over an in-memory store and the **real router** called in process, so the engine is
exercised against the routes rather than against a mock of them — and the engine cannot
behave differently under test than it does in the app, because both classify a response
through `TransportError::from_status`.

## The seq clock

`change_log.seq` is a `bigserial`, so it is assigned by `nextval()` at INSERT time while the
row becomes visible only at COMMIT time. Two concurrent transactions for one account can
therefore be assigned seq 10 and 11 and commit in the opposite order, and a client that
reads `seq > cursor`, sees 11 and advances to 11 would never receive 10 — a permanently
skipped change on that device.

Both trigger bodies (`log_change` and `log_change_by_user`, `migrations/0006_sync_functions.sql`)
therefore take `pg_advisory_xact_lock(hashtext(user_id::text))` **before** inserting the
change_log row, and hold it to commit. So for any committed row of an account every lower
seq is already committed, and a reader can only ever observe a contiguous prefix. The lock
must precede the insert; taking it afterwards is useless, because seq has already been
assigned by then. This property is the whole reason the cursor is a single integer, and it is
why nothing may compare `seq` across accounts (the sequence is global) or order by
`changed_at` where the cursor is the point.

Because the lock lives in an AFTER trigger, a multi-row transaction would acquire it *after*
its row locks, so two devices editing overlapping rows could deadlock (`40P01`). **The push
route takes the same lock as its first statement**, before it touches a row, which makes the
acquisition order uniformly advisory-then-rows everywhere; the trigger keeps its own lock as
the backstop for the dedicated REST endpoints, which write a single row and cannot invert the
order. `sync_push.rs::the_push_takes_the_advisory_lock_before_it_touches_a_row` asserts that
by observation rather than by reading the file: it holds the lock in another transaction,
waits until a push is blocked on it, and then asserts the blocked push holds no
`RowExclusiveLock` on `tasks` — a lock it would hold if it had written a row first.

The client's side of the same fact: an incremental page is complete in itself, so its cursor
commits per page and a pass that hits its page budget resumes rather than restarts, while an
initial pass commits its cursor only when it is finished, because until then the cursor is
the anchor rather than a position (A164). `docs/SYNC.md` is the normative statement of all
of this.

