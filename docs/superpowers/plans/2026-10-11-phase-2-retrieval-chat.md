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

- [ ] **Step 1: The trait**, per roadmap § 1: `complete` and `stream`, with
  `ChatRequest { model, messages, tools, temperature, max_tokens, json_mode }`,
  `ChatMsg { role, content }`, `ToolSpec`, `ChatResponse { text, tool_calls, usage }`,
  and `StreamEvent::Token(String) | ToolCall(..) | Usage { in_tokens, out_tokens,
  provider_cost_usd } | Done`. `ChatStream` is an `mpsc::Receiver<StreamEvent>`.
- [ ] **Step 2: `ScriptedLlm`** in `testing.rs`: given a list of scripted
  responses, it yields the tokens of the next one per call, records the request it
  received (so a test can assert the prompt contains the labels it should), and
  can be told to fail with `LlmError::Provider`. This is what makes the chat
  suite run without a network.
- [ ] **Step 3: Tests**: `complete` returns the scripted text; `stream` yields
  tokens in order then `Done`; a request is recorded verbatim; a scripted failure
  surfaces as `LlmError::Provider`.

**Acceptance:** `cargo test -p stvdy-core` green.

## Task 2: `crates/core/src/retrieval.rs`: RRF merge

**Files:** `crates/core/src/retrieval.rs`, `crates/core/tests/retrieval.rs`.

- [ ] **Step 1: The merge.** `rrf_merge(arms: &[Vec<RankedChunk>], k: u32, limit: usize)
  -> Vec<ScoredChunk>` where `RankedChunk { chunk_id, source_id, page, score }`.
  Score is `Σ 1/(k + rank)` over the arms a chunk appears in, `rank` 1-based
  within its arm. Ties break by `chunk_id` ascending so the order is total.
- [ ] **Step 2: Tests.** A chunk in both arms outranks one in a single arm; a
  chunk only in the weaker arm still appears (that is the point of hybrid); the
  same input twice gives the same output; `limit` truncates after scoring, never
  before; an empty arm is not an error; a duplicate id inside one arm is counted
  once.

**Acceptance:** green, and the module has no IO and no clock.

## Task 3: `crates/core/src/citations.rs`: label resolution

**Files:** `crates/core/src/citations.rs`, `crates/core/tests/citations.rs`.

- [ ] **Step 1: The resolver.** `ContextLabels` maps `[1]`..`[n]` to a chunk and
  `[W1]`.. to a web hit (Phase 3 fills that side). `resolve(text, &labels) ->
  Vec<Citation>` returns the structured citations in first-appearance order, with
  **unknown labels dropped** and each label counted once. `strip_unknown(text,
  &labels) -> String` removes the unknown markers from the text so a reply cannot
  show a citation the user cannot click.
- [ ] **Step 2: Tests**, including the golden one the roadmap asks for: a reply
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

- [ ] **Step 1: The registry** compiles `base.v1.md` with `include_str!` and
  exposes its version string (`base.v1`) for `usage_events.prompt_version`.
- [ ] **Step 2: The variable contract**, exactly as `prompts/README.md` states it:
  caller-supplied variables must be provided (an unfilled one is an **error**, not
  an empty string); optional variables take their declared default; derived
  variables are computed, never supplied. `plan_gate_instruction_derived` is
  derived from `allow_upgrade_hint` (true gives the permissive sentence, false the
  restrictive one) so a mobile build cannot be handed a contradictory pair.
- [ ] **Step 3: The untrusted wrapper.** `user_name` and `workspace_name` are
  written into the `<user_context_data>` block that `base.v1.md` already declares,
  never interpolated bare. A value containing `</user_context_data>` must not be
  able to close the block: escape it.
- [ ] **Step 4: The chat composition.** `compose_chat(vars, sources: &[PromptChunk],
  web: &[PromptWebHit]) -> Composed { text, labels }` renders the `<sources>` block
  as `[1] "Title" p.12` followed by the chunk text, returns the label map for
  Task 3, and **asserts no `{{...}}` survives** in the result.
- [ ] **Step 5: Tests.** Every declared variable is required or defaulted; a
  missing one errors with the variable's name; a derived variable cannot be
  supplied; no leftover `{{`; the escaped display name cannot break out of the
  block; a source block with zero chunks still composes (the model is told the
  workspace has none).

**Acceptance:** green. `crates/core/tests/prompts.rs` exists here and Phase 6
extends it to every module.

## Task 5: Server: retrieval, prompt assembly, credits

**Files:** `crates/server/src/chat/{retrieve.rs,prompt.rs}`, `crates/server/src/credits.rs`.

- [ ] **Step 1: `retrieve.rs`.** Two queries, both joined through `sources` and
  scoped by `user_id` and `deleted_at is null`:
  vector: `order by embedding <=> $1 limit $2` with the query embedded through
  `Embedder` with the `query: ` prefix (A2);
  keyword: `order by ts_rank(tsv, websearch_to_tsquery('simple', $1)) desc limit $2`.
  Then `rrf_merge`. An optional `source_ids` filter narrows both arms. Top-k is
  20 per arm, 12 merged.
- [ ] **Step 2: `prompt.rs`.** Builds the `ComposeChat` inputs from the user row
  (name, timezone, plan), the workspace name, and the platform, then calls the
  core composer. `now` comes from `Clock`, not `Utc::now`, so a test can pin it.
- [ ] **Step 3: `credits.rs`.** `spend(pool, user_id, action, credits) ->
  Result<i64 /* remaining */>` in one transaction: advisory lock, sum each bucket,
  reject with `AppError::Entitlement { code: "insufficient_credits" }` (402) when
  the total is short, insert the ledger row (monthly first, then purchased) and
  the `usage_events` row. `record_usage` adds the provider cost, token counts and
  `prompt_version` to the same row.
- [ ] **Step 4: `daily_chat_count`** from `usage_events` for the user's local day,
  compared against `plans.get(plan).chat_msgs_per_day` through the Phase 1 shim.
- [ ] **Step 5: Tests** for credits: two concurrent spends of the whole balance
  result in one success and one 402 (the advisory lock is the point); a monthly
  balance of 1 spends the monthly bucket, not the purchased one; the ledger sum
  equals the reported remaining.

## Task 6: Server: the chat route and SSE

**Files:** `crates/server/src/chat/{mod.rs,routes.rs,sse.rs}`,
`crates/server/src/integrations/llm.rs`, `crates/server/src/routes/mod.rs`.

- [ ] **Step 1: `integrations/llm.rs`**, the `openai_compat` impl (A1): POST
  `{LLM_BASE_URL}/chat/completions` with `stream: true`, parse `data:` frames,
  stop at `[DONE]`, map a provider error to `LlmError::Provider` (502) and a
  malformed frame to a logged skip rather than a panic. `model` comes from
  `LLM_MODEL`.
- [ ] **Step 2: The route.** `POST /workspaces/:id/chat` body
  `{message, source_ids?, use_web?}`. Order: verified user, owned workspace,
  daily cap, credit spend, persist the user message, retrieve, compose, stream.
  Emits the roadmap § 4 events in order: `token`* then `citations`, `usage`,
  `done`; `error` instead of the rest on failure. `use_web` is accepted and
  recorded but Phase 3 implements the search (answering honestly rather than
  pretending).
- [ ] **Step 3: Persistence.** The assistant message is written **after** the
  stream completes, with the resolved citations and `used_web`, so a client that
  disconnects mid-stream leaves a user message and no half-answer. The `done`
  event carries the message id.
- [ ] **Step 4: `GET /workspaces/:id/messages`**, scoped and ordered, so the
  second-device half of the exit gate is observable.
- [ ] **Step 5: Tests** (`crates/server/tests/chat.rs`) with `ScriptedLlm`:
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

- [ ] **Step 1: The fixture.** An Indonesian document about a real subject (not
  lorem ipsum), ingested through the real pipeline, plus an Indonesian question
  whose answer is in exactly one chunk.
- [ ] **Step 2: Two assertions, because the gate names two failure modes.**
  (a) The **keyword** arm alone retrieves that chunk: this is what catches a
  `tsvector` config mistake, and it runs with no model, so it always runs.
  (b) With the **real embedder** (skipped with a printed reason when the model is
  not cached, like the Phase 1 tokenizer test), the vector arm retrieves it too:
  this is what catches an embedding-prefix mistake. A prefix bug moves the query
  into a different region of the space and the wrong chunk wins.
- [ ] **Step 3: The citation resolves.** Run the scripted chat over the retrieved
  context and assert the resolved citation points at the Indonesian chunk with its
  page.
- [ ] **Step 4: Assert the tsvector config explicitly** by reading the column
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

- [ ] **Step 1: `api/sse.ts`**: `fetch` with the bearer header, a
  `ReadableStream` reader, and a parser for the named events (using
  `eventsource-parser`, already a dependency). Cancellation via `AbortController`.
- [ ] **Step 2: `useChatStream`**: sends a message, appends streamed tokens to the
  in-flight assistant turn, resolves citations on the `citations` event, and
  refetches the thread on `done`. A stream error leaves the partial text visible
  with a retry affordance rather than discarding it.
- [ ] **Step 3: The screen**: message list (user/assistant, Markdown-lite),
  citation chips that resolve to the chunk's source and page, a source-selection
  control for the `source_ids` filter, and a `use_web` toggle that says web search
  arrives in Phase 3 rather than pretending.
- [ ] **Step 4: Gate 5, accessibility**, on this screen: keyboard-only walk with a
  measured focus ring, contrast in both themes, 44px targets at phone width, no
  state by colour alone, 200% text scale. Also: the streamed answer must not move
  focus, and a screen reader must be told the answer arrived (`aria-live` on the
  message list, `aria-busy` while streaming).
- [ ] **Step 5: Gate 6's UI half**: the Indonesian fixture question typed into the
  real screen against the real server, with the answer and its citation observed.

## Task 9: Docs and the phase gate

- [ ] **Step 1: `docs/API.md`**: the chat route with its request body and the full
  SSE event list, and `GET /messages`.
- [ ] **Step 2: `docs/ARCHITECTURE.md`**: the retrieval pipeline (two arms, RRF,
  why hybrid), the citation label contract and why labels rather than UUIDs, the
  prompt composition shape for chat, and the credit spend's transaction order.
- [ ] **Step 3: `docs/ASSUMPTIONS.md`**: the Phase 2 decisions that are not
  already rows (RRF constant, top-k values, the composer landing in Phase 2
  rather than 6, the credits shim, the `use_web` deferral to Phase 3).
- [ ] **Step 4: Run the gate**: fmt, clippy, `cargo test --workspace`,
  `npm --prefix app test`, `npm run build`, the `src-tauri` trio,
  `cargo sqlx prepare --workspace --check`, and `scripts/smoke-phase2.sh`.
- [ ] **Step 5: Phase summary**, with both roadmap gates (5 and 6) stated as
  performed checks and results, not as assertions.

## Phase 2 exit checklist

- [ ] `cargo fmt --all -- --check` clean; clippy clean; `cargo test --workspace` green.
- [ ] `npm --prefix app test` and `npm run build` green; `src-tauri` trio green.
- [ ] `cargo sqlx prepare --workspace --check` exits 0 with `SQLX_OFFLINE=true`.
- [ ] A cited answer streams end to end (smoke output quoted).
- [ ] A fabricated citation label is dropped, with the golden test named.
- [ ] The assistant message is persisted with its resolved citations and is
  visible from a second device of the same account.
- [ ] An unverified account is 403; a spent-out account is 402 with nothing
  persisted; another user's workspace is 404.
- [ ] **Gate 6**: the Indonesian fixture is retrieved by the keyword arm (no
  model needed), by the vector arm (real embedder), and its citation resolves.
- [ ] **Gate 5**: the chat screen's keyboard, contrast, target-size, colour-only
  and 200% text checks are stated with their results.
- [ ] Phase summary posted; wait for "continue".

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

### Phase 2 verification (as executed)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **216 passing**, 0 failing |
| `npm --prefix app test` | **137 passing** (19 files) |
| `npm --prefix app run build` | clean (419 kB / 130 kB gzip) |
| `src-tauri` fmt + clippy + test | clean; 17 passing |
| `cargo sqlx prepare --workspace --check` (`SQLX_OFFLINE=true`) | exits 0, `.sqlx` empty |
| End-to-end smoke (`scripts/smoke-phase2.sh` + `scripts/stub-llm.py`) | PASS: an Indonesian source ingested, a streamed answer with 10 token events, **one citation from a reply that cited `[1]` and `[9]`** (the fabricated label dropped), usage matching the ledger, the labelled prompt observed at the stub, the thread visible from a second device, cross-account 404 |

### Roadmap gate 5: accessibility (chat screen)

AWAITING THE BROWSER RUN. The SPA task reports these results, and this table is
filled in from its verbatim output, not from the claim: a gate asserted from
someone else's summary is the failure mode this project already hit once. The
checks are: keyboard-only reach with a measured focus ring, contrast in both
themes, 44px targets at phone width, no state by colour alone, 200% text scale,
and the two streaming-specific ones (the arriving answer must not move focus, and
`aria-live` must announce it).

### Roadmap gate 6: Indonesian end to end

| Half | Result |
|---|---|
| Keyword arm retrieves the Indonesian chunk (no model) | **PASS**, and it runs unconditionally |
| The `tsvector` column is generated with `simple`, asserted against the database | **PASS** (a stemming config would fail here) |
| Vector arm retrieves it with the **real** embedder | **PASS** (6.2 s, real model) |
| `embed_query` applies the model's query convention | **PASS**, and it caught the missing override (execution note 1) |
| The citation resolves to the Indonesian source | **PASS** |
| The UI half: an Indonesian question typed into the real screen | AWAITING THE BROWSER RUN (same SPA task as gate 5 above) |

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
