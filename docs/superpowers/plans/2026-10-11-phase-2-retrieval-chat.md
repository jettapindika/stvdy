# Phase 2. Retrieval and streaming chat. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`.
> This plan implements them; it never redefines them. Where this plan and the
> roadmap disagree, the roadmap wins and this file is the bug.

**Goal:** a question about a workspace's sources produces a streamed answer whose
every claim is cited to a real chunk, with citations that cannot be forged, and
the exchange is persisted so the user's other device sees it. Retrieval is
hybrid (vector + keyword, RRF-merged) because a study app is asked for exact
terms as often as for meaning.

**Exit gate (observable, from the roadmap):** a cited answer streams, unknown
chunk ids are dropped, and the message is persisted and visible on a second
device.

**Two roadmap gates start here and are not optional:**
- Gate 5, accessibility: every phase with UI changes states the check performed
  and the result.
- Gate 6, Indonesian end to end: an Indonesian fixture plus an Indonesian query
  must retrieve the correct chunk, answer from it, and resolve the citation.
  This is the gate that catches a `tsvector` config or embedding-prefix mistake,
  both of which fail *silently* for Indonesian while English tests stay green.

---

## Decisions this plan pins (so parallel work cannot diverge)

1. **`LlmProvider` lands in `crates/core/src/traits/llm.rs`** exactly as the
   roadmap § Cross-Cutting Contracts § 1 specifies, plus one addition the SSE
   contract needs: `StreamEvent::Usage { in_tokens, out_tokens, provider_cost_usd }`
   so `usage_events` can carry the cost (A41) without the server guessing from
   token counts.
2. **The prompt composer is built here, not in Phase 6.** Phase 6 adds the
   strict-JSON trimming and `crates/core/tests/prompts.rs` for every module; chat
   needs the other composition shape (base plus sources) now, and the roadmap
   forbids inline prompt text. So `crates/core/src/prompts/{mod,registry}.rs`
   lands in Phase 2 with: `include_str!` registry, the `{{variable}}` contract
   (caller-supplied, derived, optional), the untrusted-value wrapper, and the
   assertion that no `{{...}}` survives. Phase 6 adds `base_sections_for` and the
   full module sweep.
3. **RRF is a pure function in `crates/core/src/retrieval.rs`.** Standard
   `score = Σ 1/(k + rank)` with `k = 60`, deduplicating a chunk that appears in
   both lists and summing its contributions. Ties break by chunk id, so two runs
   of the same query return the same order. No SQL, no clock, no IO.
4. **Citation resolution is a pure function in `crates/core/src/citations.rs`.**
   The model emits `[1]`, `[2]` for chunks (and `[W1]` from Phase 3 for web). The
   server keeps a per-request `label -> chunk` map, resolves what it sent, and
   **drops anything else** (A17). Labels are never persisted; `messages.citations`
   stores the resolved structured array. A fabricated label is dropped, not
   rewritten, and a golden test asserts it.
5. **Retrieval is SQL in the server, not in `core`.** `chunks` has no `user_id`
   (migration 0007), so ownership is `chunks.source_id -> sources.user_id`, and
   every retrieval query joins through `sources` with `deleted_at is null`. A
   query without that join is a cross-tenant read, which is the threat model's
   "read another user's rows" row.
6. **The keyword arm uses the `simple` tsvector config** that migration 0007
   already generated. `simple` does not stem, which is what makes Indonesian
   work; a switch to `english` would silently break gate 6 while every English
   test stayed green. Phase 2 asserts the config in a test rather than trusting
   the column definition.
7. **Credits: a minimal, pinned spend now, generalised in Phase 7.** Chat is the
   first route that spends, and the SSE contract carries a `usage` event, so
   Phase 2 implements the spend exactly as roadmap § 3 pins it: one transaction,
   `pg_advisory_xact_lock(hashtext(user_id::text))`, read bucket sums, reject if
   insufficient, insert a negative `credit_ledger` row **and** a `usage_events`
   row. Monthly bucket first, then purchased (A9). Phase 7 adds the plan matrix,
   the counters and the `check` engine on top; `crates/server/src/credits.rs` is
   that seam and says so.
8. **Daily message cap** is counted from `usage_events` for the user's local
   day (`chrono-tz`, A6), not from a counter column: a counter can drift and the
   drift is invisible until a user is wrongly blocked.
9. **The chat route is `VerifiedUser`** (A43: chat is one of the four locked
   actions), and takes the same `AuthUser`-derived plan as everything else.
10. **SSE is written by hand** (`axum::response::sse`), never `EventSource` on the
    client: chat is a POST with an `Authorization` header, and `EventSource`
    supports neither. A comment heartbeat every 15s keeps proxies from closing an
    idle stream.
11. **`GET /workspaces/:id/messages`** returns the persisted thread, ordered by
    `created_at`, scoped by `user_id`, tombstones excluded. This is what proves
    "visible on a second device" in the exit gate.
12. **The LLM integration is `openai_compat`** (A1): base URL, key and model from
    env, `/chat/completions` with `stream: true`, parsing `data:` frames and
    tolerating `[DONE]`. It must not assume a vendor-specific field.

---

## File structure (new this phase)

```
crates/core/src/
  traits/llm.rs                         # LlmProvider, ChatRequest, StreamEvent
  retrieval.rs                          # RRF merge, pure
  citations.rs                          # label -> chunk resolver, pure
  prompts/{mod.rs,registry.rs}          # include_str! registry + composer
crates/core/tests/{retrieval.rs,citations.rs,prompts.rs}
crates/server/src/
  integrations/llm.rs                   # openai_compat streaming impl
  credits.rs                            # Phase 2 spend shim (Phase 7 generalises)
  chat/{mod.rs,routes.rs,sse.rs,retrieve.rs,prompt.rs}
crates/server/tests/{chat.rs,indonesian.rs}
app/src/features/chat/{ChatScreen.tsx,useChatStream.ts,MessageList.tsx,CitationChip.tsx}
scripts/smoke-phase2.sh                 # stub LLM + real server, end to end
```

---

## Task 1: `crates/core`: `LlmProvider` + a scripted double

**Files:** `crates/core/src/traits/{llm.rs,testing.rs}`, `crates/core/tests/llm.rs`.

- [x] **Step 1: The trait**, per roadmap § 1: `complete` and `stream`, with
  `ChatRequest { model, messages, tools, temperature, max_tokens, json_mode }`,
  `ChatMsg { role, content }`, `ToolSpec`, `ChatResponse { text, tool_calls, usage }`,
  and `StreamEvent::Token(String) | ToolCall(..) | Usage { in_tokens, out_tokens,
  provider_cost_usd } | Done`. `ChatStream` is an `mpsc::Receiver<StreamEvent>`.
- [x] **Step 2: `ScriptedLlm`** in `testing.rs`: given a list of scripted
  responses, it yields the tokens of the next one per call, records the request it
  received (so a test can assert the prompt contains the labels it should), and
  can be told to fail with `LlmError::Provider`. This is what makes the chat
  suite run without a network.
- [x] **Step 3: Tests**: `complete` returns the scripted text; `stream` yields
  tokens in order then `Done`; a request is recorded verbatim; a scripted failure
  surfaces as `LlmError::Provider`.

**Acceptance:** `cargo test -p stvdy-core` green.

## Task 2: `crates/core/src/retrieval.rs`: RRF merge

**Files:** `crates/core/src/retrieval.rs`, `crates/core/tests/retrieval.rs`.

- [x] **Step 1: The merge.** `rrf_merge(arms: &[Vec<RankedChunk>], k: u32, limit: usize)
  -> Vec<ScoredChunk>` where `RankedChunk { chunk_id, source_id, page, score }`.
  Score is `Σ 1/(k + rank)` over the arms a chunk appears in, `rank` 1-based
  within its arm. Ties break by `chunk_id` ascending so the order is total.
- [x] **Step 2: Tests.** A chunk in both arms outranks one in a single arm; a
  chunk only in the weaker arm still appears (that is the point of hybrid); the
  same input twice gives the same output; `limit` truncates after scoring, never
  before; an empty arm is not an error; a duplicate id inside one arm is counted
  once.

**Acceptance:** green, and the module has no IO and no clock.

## Task 3: `crates/core/src/citations.rs`: label resolution

**Files:** `crates/core/src/citations.rs`, `crates/core/tests/citations.rs`.

- [x] **Step 1: The resolver.** `ContextLabels` maps `[1]`..`[n]` to a chunk and
  `[W1]`.. to a web hit (Phase 3 fills that side). `resolve(text, &labels) ->
  Vec<Citation>` returns the structured citations in first-appearance order, with
  **unknown labels dropped** and each label counted once. `strip_unknown(text,
  &labels) -> String` removes the unknown markers from the text so a reply cannot
  show a citation the user cannot click.
- [x] **Step 2: Tests**, including the golden one the roadmap asks for: a reply
  citing `[1]` and `[7]` against a context of three chunks yields exactly one
  citation, and `[7]` is absent from both the citations and the stripped text. A
  label repeated three times yields one citation. `[10]` against a context of two
  is dropped. A malformed marker (`[`, `[1`, `[]`, `[W]`) is left alone, not
  panicked on.

**Acceptance:** green, pure, and the golden test names the fabricated label in its
failure message.

## Task 4: `crates/core/src/prompts/`: composer and registry

**Files:** `crates/core/src/prompts/{mod.rs,registry.rs}`,
`crates/core/tests/prompts.rs`.

- [x] **Step 1: The registry** compiles `base.v1.md` with `include_str!` and
  exposes its version string (`base.v1`) for `usage_events.prompt_version`.
- [x] **Step 2: The variable contract**, exactly as `prompts/README.md` states it:
  caller-supplied variables must be provided (an unfilled one is an **error**, not
  an empty string); optional variables take their declared default; derived
  variables are computed, never supplied. `plan_gate_instruction_derived` is
  derived from `allow_upgrade_hint` (true gives the permissive sentence, false the
  restrictive one) so a mobile build cannot be handed a contradictory pair.
- [x] **Step 3: The untrusted wrapper.** `user_name` and `workspace_name` are
  written into the `<user_context_data>` block that `base.v1.md` already declares,
  never interpolated bare. A value containing `</user_context_data>` must not be
  able to close the block: escape it.
- [x] **Step 4: The chat composition.** `compose_chat(vars, sources: &[PromptChunk],
  web: &[PromptWebHit]) -> Composed { text, labels }` renders the `<sources>` block
  as `[1] "Title" p.12` followed by the chunk text, returns the label map for
  Task 3, and **asserts no `{{...}}` survives** in the result.
- [x] **Step 5: Tests.** Every declared variable is required or defaulted; a
  missing one errors with the variable's name; a derived variable cannot be
  supplied; no leftover `{{`; the escaped display name cannot break out of the
  block; a source block with zero chunks still composes (the model is told the
  workspace has none).

**Acceptance:** green. `crates/core/tests/prompts.rs` exists here and Phase 6
extends it to every module.

## Task 5: Server: retrieval, prompt assembly, credits

**Files:** `crates/server/src/chat/{retrieve.rs,prompt.rs}`, `crates/server/src/credits.rs`.

- [x] **Step 1: `retrieve.rs`.** Two queries, both joined through `sources` and
  scoped by `user_id` and `deleted_at is null`:
  vector: `order by embedding <=> $1 limit $2` with the query embedded through
  `Embedder` with the `query: ` prefix (A2);
  keyword: `order by ts_rank(tsv, websearch_to_tsquery('simple', $1)) desc limit $2`.
  Then `rrf_merge`. An optional `source_ids` filter narrows both arms. Top-k is
  20 per arm, 12 merged.
- [x] **Step 2: `prompt.rs`.** Builds the `ComposeChat` inputs from the user row
  (name, timezone, plan), the workspace name, and the platform, then calls the
  core composer. `now` comes from `Clock`, not `Utc::now`, so a test can pin it.
- [x] **Step 3: `credits.rs`.** `spend(pool, user_id, action, credits) ->
  Result<i64 /* remaining */>` in one transaction: advisory lock, sum each bucket,
  reject with `AppError::Entitlement { code: "insufficient_credits" }` (402) when
  the total is short, insert the ledger row (monthly first, then purchased) and
  the `usage_events` row. `record_usage` adds the provider cost, token counts and
  `prompt_version` to the same row.
- [ ] **Step 4: `daily_chat_count`** from `usage_events` for the user's local day,
  compared against `plans.get(plan).chat_msgs_per_day` through the Phase 1 shim. — **the property is real and measured; the named artefact is not, and it was superseded by design, so the box stays unticked.** `daily_chat_count` does not exist at HEAD and neither does "the Phase 1 shim": Phase 7's `entitlements::check` replaced both (A85, A146), and the count now lives in the usage snapshot as `chat_msgs_today` — `crates/server/src/entitlements.rs`'s one statement, `count(*) from usage_events where action = 'chat_message' and created_at >= local_day_start(timezone, clock)`. **Both halves are asserted, and the local-day half is the stronger test:** `crates/server/tests/entitlements.rs::the_snapshot_counts_the_same_rows_the_daily_cap_counts` gives two accounts the *same three rows* under `Pacific/Kiritimati` (+14) and `America/New_York` (−4), asserts the snapshot reports `cap` for one and `cap - 3` for the other, and then drives the cap itself — a UTC-midnight boundary would report the cap for both, which is exactly the Jakarta-at-07:00 bug A6 names. The comparison against `plans.get(plan).chat_msgs_per_day` is `crates/core/src/entitlements/check.rs`'s `counted(f, cfg.chat_msgs_per_day, u.chat_msgs_today, plan)`, and the end-to-end refusal is `crates/server/tests/chat.rs::the_daily_message_cap_trips` (the free plan's number, read from config rather than hardcoded). Left unticked for the reason Phase 4's `limits::enforce_batch` box is: **a tick for a superseded step hides the supersession**, and this project has been bitten by that shape twice.
- [x] **Step 5: Tests** for credits: two concurrent spends of the whole balance
  result in one success and one 402 (the advisory lock is the point); a monthly
  balance of 1 spends the monthly bucket, not the purchased one; the ledger sum
  equals the reported remaining.

## Task 6: Server: the chat route and SSE

**Files:** `crates/server/src/chat/{mod.rs,routes.rs,sse.rs}`,
`crates/server/src/integrations/llm.rs`, `crates/server/src/routes/mod.rs`.

- [x] **Step 1: `integrations/llm.rs`**, the `openai_compat` impl (A1): POST
  `{LLM_BASE_URL}/chat/completions` with `stream: true`, parse `data:` frames,
  stop at `[DONE]`, map a provider error to `LlmError::Provider` (502) and a
  malformed frame to a logged skip rather than a panic. `model` comes from
  `LLM_MODEL`.
- [x] **Step 2: The route.** `POST /workspaces/:id/chat` body
  `{message, source_ids?, use_web?}`. Order: verified user, owned workspace,
  daily cap, credit spend, persist the user message, retrieve, compose, stream.
  Emits the roadmap § 4 events in order: `token`* then `citations`, `usage`,
  `done`; `error` instead of the rest on failure. `use_web` is accepted and
  recorded but Phase 3 implements the search (answering honestly rather than
  pretending).
- [x] **Step 3: Persistence.** The assistant message is written **after** the
  stream completes, with the resolved citations and `used_web`, so a client that
  disconnects mid-stream leaves a user message and no half-answer. The `done`
  event carries the message id.
- [x] **Step 4: `GET /workspaces/:id/messages`**, scoped and ordered, so the
  second-device half of the exit gate is observable.
- [x] **Step 5: Tests** (`crates/server/tests/chat.rs`) with `ScriptedLlm`:
  a scripted answer citing `[1]` and `[9]` yields a `citations` event with exactly
  the chunk for `[1]`; the persisted assistant message has that citation and not
  `[9]`; the prompt the model received contains the label block (asserted on the
  recorded request); the second `GET /messages` from another device of the same
  user sees both turns; another user's workspace is 404; an unverified account is
  403 `email_not_verified`; a spent-out account is 402 `insufficient_credits`
  **and no message was persisted**; the `usage` event's `credits_remaining`
  matches the ledger.

**Acceptance:** the suite passes and its output is quoted in the phase summary.

## Task 7: Indonesian end-to-end gate (roadmap gate 6)

**Files:** `crates/server/tests/indonesian.rs`.

- [x] **Step 1: The fixture.** An Indonesian document about a real subject (not
  lorem ipsum), ingested through the real pipeline, plus an Indonesian question
  whose answer is in exactly one chunk.
- [x] **Step 2: Two assertions, because the gate names two failure modes.**
  (a) The **keyword** arm alone retrieves that chunk: this is what catches a
  `tsvector` config mistake, and it runs with no model, so it always runs.
  (b) With the **real embedder** (skipped with a printed reason when the model is
  not cached, like the Phase 1 tokenizer test), the vector arm retrieves it too:
  this is what catches an embedding-prefix mistake. A prefix bug moves the query
  into a different region of the space and the wrong chunk wins.
- [x] **Step 3: The citation resolves.** Run the scripted chat over the retrieved
  context and assert the resolved citation points at the Indonesian chunk with its
  page.
- [x] **Step 4: Assert the tsvector config explicitly** by reading the column
  definition, so a later migration that switched it to `english` fails here rather
  than in production.

## Task 8: SPA: the chat screen

**Files:** `app/src/features/chat/*`, `app/src/api/sse.ts`, `app/src/router.tsx`,
`app/src/features/workspaces/WorkspaceScreen.tsx`.

The chat screen lives in the workspace view's existing `chat` panel, not at its
own route: `WorkspaceScreen` already owns the ARIA tab strip with a `chat`
panel rendering `NotBuiltYet`, and its roving-tabindex keyboard contract is
tested. Making the panel URL-driven would turn `active` into a route param, which
is a bigger change than this phase needs. "Wire the route" below means that panel.

- [x] **Step 1: `api/sse.ts`**: `fetch` with the bearer header, a
  `ReadableStream` reader, and a parser for the named events (using
  `eventsource-parser`, already a dependency). Cancellation via `AbortController`.
- [x] **Step 2: `useChatStream`**: sends a message, appends streamed tokens to the
  in-flight assistant turn, resolves citations on the `citations` event, and
  refetches the thread on `done`. A stream error leaves the partial text visible
  with a retry affordance rather than discarding it.
- [x] **Step 3: The screen**: message list (user/assistant, Markdown-lite),
  citation chips that resolve to the chunk's source and page, a source-selection
  control for the `source_ids` filter, and a `use_web` toggle that says web search
  arrives in Phase 3 rather than pretending.
- [x] **Step 4: Gate 5, accessibility**, on this screen: keyboard-only walk with a
  measured focus ring, contrast in both themes, 44px targets at phone width, no
  state by colour alone, 200% text scale. Also: the streamed answer must not move
  focus, and a screen reader must be told the answer arrived (`aria-live` on the
  message list, `aria-busy` while streaming).
- [x] **Step 5: Gate 6's UI half**: the Indonesian fixture question typed into the
  real screen against the real server, with the answer and its citation observed.

## Task 9: Docs and the phase gate

- [x] **Step 1: `docs/API.md`**: the chat route with its request body and the full
  SSE event list, and `GET /messages`.
- [x] **Step 2: `docs/ARCHITECTURE.md`**: the retrieval pipeline (two arms, RRF,
  why hybrid), the citation label contract and why labels rather than UUIDs, the
  prompt composition shape for chat, and the credit spend's transaction order.
- [x] **Step 3: `docs/ASSUMPTIONS.md`**: the Phase 2 decisions that are not
  already rows (RRF constant, top-k values, the composer landing in Phase 2
  rather than 6, the credits shim, the `use_web` deferral to Phase 3).
- [x] **Step 4: Run the gate**: fmt, clippy, `cargo test --workspace`,
  `npm --prefix app test`, `npm run build`, the `src-tauri` trio,
  `cargo sqlx prepare --workspace --check`, and `scripts/smoke-phase2.sh`.
- [x] **Step 5: Phase summary**, with both roadmap gates (5 and 6) stated as
  performed checks and results, not as assertions.

## Phase 2 exit checklist

- [x] `cargo fmt --all -- --check` clean; clippy clean; `cargo test --workspace` green.
- [x] `npm --prefix app test` and `npm run build` green; `src-tauri` trio green.
- [x] `cargo sqlx prepare --workspace --check` exits 0 with `SQLX_OFFLINE=true`.
- [x] A cited answer streams end to end (smoke output quoted).
- [x] A fabricated citation label is dropped, with the golden test named.
- [x] The assistant message is persisted with its resolved citations and is
  visible from a second device of the same account.
- [x] An unverified account is 403; a spent-out account is 402 with nothing
  persisted; another user's workspace is 404.
- [x] **Gate 6**: the Indonesian fixture is retrieved by the keyword arm (no
  model needed), by the vector arm (real embedder), and its citation resolves.
- [x] **Gate 5**: the chat screen's keyboard, contrast, target-size, colour-only
  and 200% text checks are stated with their results.
- [x] Phase summary posted; wait for "continue".

## Execution notes (Phase 2, as built)

Deviations and discoveries from running the plan.

1. **`Embedder::embed_query` had to be added, and the default was a real defect.**
   The trait gained `embed_query` (defaulting to `embed`) so a model's query
   convention is the model's business rather than a prefix hardcoded at the call
   site. The fastembed impl then shipped without overriding it, so every search
   query was embedded with the `passage: ` prefix. E5 puts a query and a passage
   in different regions of the space, so retrieval returned the wrong chunk with
   no error. The Indonesian gate caught it, because that test asserts the two
   embeddings of the same text differ; no English test would have failed.
2. **The test harness embedder made gate 6's vector half vacuous.**
   `TestState` uses `HashEmbedder` (deterministic, no download), which ignores the
   prefix. Asserting the vector arm against it would have passed with the defect
   above still present. The vector half now builds the real `FastembedEmbedder`
   and skips with a printed reason when the model is not cached, which is the same
   pattern as the Phase 1 tokenizer test.
3. **A fresh account had zero credits and could not send a message.** Every chat
   test was a 402. The monthly allowance is now granted at signup (A64): the
   recurring `monthly_grant` job is Phase 7's, and a job that runs at period start
   cannot serve the account whose period starts at signup.
4. **`ts_rank` returns `real`, not `double precision`.** sqlx will not decode
   float4 into f64, so the keyword arm 500'd on every query. The cast is now
   explicit and commented.
5. **A pasted-text source has no page.** The citation's `page` is `null` for text
   and the real page for a PDF. Three tests asserted `1` because they were written
   against an assumption rather than the data; all three now assert the truth,
   which is the more useful check.
6. **Two source-scanning tests failed on themselves.** The assertions forbid
   certain literals and scanned the whole file, so they matched their own
   assertion messages. Both now scan only the production half (split at
   `#[cfg(test)]`) and build the forbidden literal at run time.
7. **The smoke run needs a stub that speaks HTTP, not a trait double.** The Rust
   suite replaces the provider at the trait boundary, which cannot catch a
   wire-format mistake in the request body or the SSE frame parser. `scripts/
   stub-llm.py` serves real HTTP and real SSE and records every request body, so
   the smoke run asserts the prompt carried the label block.
8. **The smoke run's ledger check was unscoped.** It summed `credit_ledger` across
   every account the previous runs had created, so it compared one user's balance
   against everyone's. Scoped to the account under test.
9. **The cross-account probe was testing the wrong rule.** The intruder was
   unverified, so A43's 403 fired before the ownership check could be reached.
   The probe now verifies the intruder's email first, which leaves workspace
   ownership as the only thing that can refuse them.
10. **Dropping a label left its whitespace behind.** The browser run rendered
    "and also ." where the model had written "and also [9].", because the marker
    was removed and the space it occupied was not. `strip_unknown` now tidies the
    removal seam: a doubled space collapses and a space before sentence
    punctuation is dropped, while a label that *stays* is left byte-for-byte as
    written so the client does not have to re-find it. The one existing test that
    pinned the double space was asserting the artifact rather than the intent and
    now asserts a clean sentence plus the absence of any surviving marker.

### Review round: defects found after the first Phase 2 summary

A review of the first summary produced findings that were checked against the tree
rather than taken on faith. Eleven were real and are fixed here.

11. **A rejected citation was visible to the user while the answer streamed.** The
    token loop yielded every token verbatim and stripped only the *stored* copy, so
    a fabricated `[9]` was on screen from the token carrying it until `done`
    replaced the live turn with the refetched copy. `docs/API.md` claims a reply
    "can never show a citation the user cannot open", which was false on the
    streaming path. The server now holds back at most one marker's length
    (`citations::streamable_end`) and emits a marker only once it is complete and
    known, so the fabricated one is dropped before the user sees it and the
    client's unmatched-marker branch becomes unreachable. The hold-back has four
    clauses and each one is load-bearing: an unterminated `[`, the space before it,
    a trailing space on its own, and a complete marker at the very end of the text.
    The last one is subtle: `strip_unknown` decides whether to swallow the space
    before a marker by looking at the byte after it, and at the end of a slice that
    byte is not known yet, so the streamed text joined two words where the stored
    text kept them apart. `strip_unknown` is now the single function used on both
    paths, and it drops the space before a marker only when the marker is not
    followed by a word character.
12. **The gate-6 UI row recorded a false PASS.** It quoted
    `log.textContent.includes('[9]') === false`, sampled *after* `done`, so it
    measured the persisted copy rather than the streaming path. Replaced with the
    token-event assertion above, and the smoke run now samples the stream itself.
13. **An empty answer left the stream unterminated.** The branch only logged, so a
    client waited for a `done` that never came, since `done` is the turn's end.
    Every exit path now emits exactly one terminator.
14. **`use_web` was accepted and then claimed.** The flag reached the prompt as
    "Web search: on" with no web context, was stored as `used_web: true`, and
    invited the model to say it had searched. Now refused with 400 `validation`
    until Phase 3 implements the arm (A65 is superseded).
15. **A server with no provider spent a credit per request.** The `state.llm`
    check sat after the spend and after the user message was written, so a fresh
    checkout (whose `.env.example` has no `LLM_BASE_URL`) charged for a request
    that could never succeed. The check now runs before the spend, and retrieval
    and composition were moved ahead of it too: they are a database read and a
    pure function, so doing them first costs nothing and shrinks the window in
    which a credit can be lost.
16. **`insufficient_credits` put the wrong numbers in `limit`/`used`.** Those
    fields carry the plan-matrix sense, so a client rendering them would print
    "the limit is 1, you have 59" on a 402, and `min_plan` was `None`, so a
    spent-out user was offered no upgrade at all. Now `limit`/`used` are null (the
    numbers are in the message) and `min_plan` names the plan with the bigger
    allowance, or `None` on the top plan.
17. **A blank env value defeated its own default.** `get_or` used
    `unwrap_or_else`, so `LLM_MODEL=` (which the template ships blank) produced an
    empty model name and the default never fired: the documented way to enable
    chat, setting `LLM_BASE_URL` alone, would have posted `"model": ""` and taken
    a credit before the provider's 400. An empty value now counts as absent, which
    is the pattern the rest of the file already used with `.filter(|s| !s.is_empty())`.
18. **`HttpEmbedder` had no query convention.** It inherited the trait default,
    which is `embed`, so pointing `EMBEDDER=http` at an E5 endpoint reproduced the
    exact silent degradation the Indonesian gate had just caught on the fastembed
    side. It now takes `EMBEDDER_QUERY_PREFIX` and applies it in `embed_query`.
    The now-dead free function `embed_query(model, text)` was deleted: it applied
    the prefix unconditionally, which is the second divergent path the trait method
    exists to remove.
19. **The client's post-stream refetch had no `.catch()`.** TanStack Query's
    `invalidateQueries` rejects when the refetch fails, so a network blip at the
    moment the answer completed left an unhandled rejection and never cleared the
    live turn, which kept the raw-token copy (and any marker the server had
    dropped) on screen indefinitely.
20. **Gate 6 could not catch a regression in the retrieval path.** Its vector
    assertion ran its own SQL against `embed_query`, while the chat half ran
    through `retrieve` with the harness's hash embedder, where `embed` and
    `embed_query` are identical by definition. A regression inside `retrieve`
    would have passed everything. There is now a `TestState::with_embedder` seam,
    and a `ConventionRecorder` test asserts that `retrieve` calls `embed_query` and
    never `embed`. Proved non-vacuous by re-injecting the regression: it fails.
21. **The smoke run could not be re-run by anyone else.** It drove an
    already-running server but never said how to start it, and the server's
    `LLM_BASE_URL` had to point at the stub's port with nothing in the repo
    recording the pairing, so a re-runner saw an opaque 502 (or a burned credit).
    Three `justfile` recipes now state it, and the script diagnoses a missing
    server and an unconfigured provider by name.

### Phase 2 verification (as executed)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **230 passing**, 0 failing |
| `npm --prefix app test` | **137 passing** (19 files) |
| `npm --prefix app run build` | clean (419 kB / 130 kB gzip) |
| `src-tauri` fmt + clippy + test | clean; 17 passing |
| `cargo sqlx prepare --workspace --check` (`SQLX_OFFLINE=true`) | exits 0, `.sqlx` empty |
| End-to-end smoke (`scripts/smoke-phase2.sh` + `scripts/stub-llm.py`) | PASS: an Indonesian source ingested, a streamed answer with 10 token events, **one citation from a reply that cited `[1]` and `[9]`** (the fabricated label dropped), usage matching the ledger, the labelled prompt observed at the stub, the thread visible from a second device, cross-account 404 |

### Roadmap gate 5: accessibility (chat screen)

Method: the built bundle in headless Chromium, measured with `getComputedStyle`
and `getBoundingClientRect` after real key presses. Not a stylesheet reading.

**Cost of the target-size fix, recorded rather than only the pass:** the citation
chip's phone-width rule (`min-height: 2.75rem`) makes the line holding a citation
taller at phone width. An absolute-positioned hit area was rejected because the
box would stay 15px, which passes only an audit that measures something other than
the target.

| Requirement | Result |
|---|---|
| Keyboard-only reach, visible focus ring | PASS. Tab-only walk at 1280px: citation chip x4, source checkbox, composer, Ask (with a draft), then out of the panel. Every stop has a 2px solid `rgb(53,136,200)` ring at 2px offset (the composer 1px); ring against its ground is 3.56:1 light and 8.19:1 dark. The disabled Ask button and the disabled web toggle are correctly *not* tab stops |
| Contrast, default dark **and** light themes | PASS, 0 failures. Computed per text node against each element's composited effective background, in both themes. Worst cases: light chip text 5.04:1 at 10.5px and panel label 5.06:1; dark chip text 7.1:1 and panel label 7.41:1 |
| Targets >= 44px at phone width (390px) | **ONE REAL FAILURE FOUND AND FIXED.** `button.cite` measured 118x15px. A `max-width: 767px` rule (min-height 2.75rem, 0.4rem/0.8rem padding) took it to 133x44. Passing: composer 358x80, Ask 61x44, source row label 358x44, web-search label 142x44. Cost, stated: the line holding a citation is taller at phone width. An absolute-positioned hit area was rejected because the box would stay 15px, passing only an audit that measures something other than the target |
| No state conveyed by colour alone | PASS. Citation = the word plus an `aria-label`; streaming = `role="status"` "Searching this workspace"; disabled web toggle = a label plus `aria-describedby`; source selection = a worded count; errors = title and reason in `role="alert"` |
| 200% text scale without clipping or overlap | PASS. `fontSize: 32px` doubled every computed size exactly (12 to 24px) at both 390 and 1280. 0 clipped, 0 overflowing the panel, `scrollWidth == clientWidth` (390/390, 1280/1280); the composer clears the fixed tab bar at full scroll (form bottom 755 = tab bar top 755) |
| Streaming: the arriving answer must not move focus | PASS. Focus stayed on `#chat-message` in 120 of 120 samples at 10ms intervals |
| Streaming: a screen reader is told it arrived | PASS structurally: the list is `role="log"` with `aria-live="polite"` and an `aria-label`, and `aria-busy` traced false to true to false with the first transition 11ms after submit |

Not checked, stated rather than implied: no real screen reader was run (the live
region is asserted structurally, the announcement itself is not observed);
`prefers-reduced-motion` was not exercised; the citation panel was measured at
200% only at 1280px, not at phone width.

### Roadmap gate 6: Indonesian end to end

| Half | Result |
|---|---|
| Keyword arm retrieves the Indonesian chunk (no model) | **PASS**, and it runs unconditionally |
| The `tsvector` column is generated with `simple`, asserted against the database | **PASS** (a stemming config would fail here) |
| Vector arm retrieves it with the **real** embedder | **PASS** (6.2 s, real model) |
| `embed_query` applies the model's query convention | **PASS**, and it caught the missing override (execution note 1) |
| The citation resolves to the Indonesian source | **PASS** |
| The UI half: an Indonesian question typed into the real screen | **PASS**, with a corrected measurement. Account created through the UI and verified from the `var/mail` link; workspace "Jaringan Komputer"; the Indonesian fixture ingested to `ready` (1 chunk); the question typed into the chat panel. Exactly **one** citation chip rendered, labelled "Open source: Topologi Jaringan, page unknown", and clicking it opened a panel with the fixture chunk text. "page unknown" is honest: pasted text has no pages |
| The streamed text, sampled **while the stream is open** | **PASS, and this row replaces a false one.** The first browser run sampled `log.textContent` *after* `done`, so it recorded the persisted copy and reported a pass the streaming path did not earn: the token loop yielded every token verbatim, so `[9]` was on screen from the token that carried it until the refetch replaced the live turn. The server now holds back a partial marker and drops an unknown one before emitting, and the smoke run asserts it on the token events: `'Mitochondria produce ATP through oxidative phosphorylation [1], and also .'`, with `[9]` absent and the sent label present |
| The pre-stream error path, against the real server | **PASS.** With the stub provider down, the 502 arrived as a JSON body *before* the stream opened and the screen rendered it as "Service unavailable" with an "Ask again" retry and no assistant turn. A distinct case from a mid-stream `error`, where the user's turn is stored |

**Boxes left unticked, and why.**

  - Task 5 Step 4, "`daily_chat_count` from `usage_events` for the user's local day, compared against `plans.get(plan).chat_msgs_per_day` through the Phase 1 shim" -- the daily cap itself is real and enforced (`crates/server/src/credits.rs` reads `chat_msgs_per_day`, and `GET /usage` reports it with its used count from `crates/server/src/usage.rs`), but the function this box names does not exist at HEAD: Phase 7 replaced the Phase 1 shim with the entitlement snapshot (`UsageSnapshot::chat_msgs_today`), which this plan's own Self-Review already assigns to Phase 7 ("the counters are Phase 7"), so the named artefact cannot be confirmed.

## Self-Review

- **Spec coverage.** SPEC § Retrieval and chat is covered: hybrid vector plus
  keyword with RRF, optional source scoping, chunk-labelled prompt, citation
  validation that drops unknown ids, streaming over SSE with a final structured
  citations event, persisted messages, and "say so instead of guessing" (the base
  prompt's rule 2 plus a test that a workspace with no sources composes and the
  model is told). `use_web` is accepted and recorded but its search arm is Phase 3,
  which this plan states rather than fakes.
- **Deliberate boundaries.** Tools and the agent are Phase 12. Web search is
  Phase 3. The full entitlements engine, the plan matrix and the counters are
  Phase 7; Phase 2 implements only the pinned credit spend, in one file, marked as
  the seam.
- **Risk.** The largest is that a test double and the code share an assumption
  (exactly the Phase 1 defect that reached the tree). Mitigation: the chat suite
  asserts on the *recorded request* the provider received, not only on the
  response path, and the smoke run drives a stub HTTP server over a real socket so
  the wire format is exercised rather than mocked at the trait boundary.
- **Type consistency.** `LlmProvider`, `ChatRequest`, `StreamEvent`,
  `ScoredChunk`, `ContextLabels`, `Citation`, `Composed`, `spend` are each
  defined once and referenced by name thereafter.
