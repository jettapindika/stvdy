# Phase 4. Flashcards and spaced repetition. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`.
> This plan implements them; it never redefines them. Where this plan and the
> roadmap disagree, the roadmap wins and this file is the bug.

**Goal:** a student selects sources, gets a deck of cards generated from them with
every card linked back to the chunk it came from, reviews the deck with four grades,
and two devices reviewing the same card end up with byte-identical `ease`,
`interval_days` and `due_at`.

**Architecture:** the card's scheduling state is **derived**, never authored. A
review is an append-only fact (`flashcard_reviews`); the card's state is a pure
function of its reviews replayed in `(reviewed_at, id)` order
(`crates/core/src/sm2.rs`). Generation is one `generate_flashcards` job: strict-JSON
prompt composition (`crates/core/src/prompts/`, built here), one retry on a parse
failure, and a refund if the job fails for good. The SPA adds a Studio panel to
generate a deck and a study screen to review one.

**Tech Stack:** Rust (core + server, no new crates), Postgres/sqlx, React + Vite +
TypeScript + Tailwind, TanStack Query.

**Exit gate (observable, from the roadmap):** a deck is generated from selected
sources; reviews arriving from two clients replay to identical ease/interval/due.

**Two roadmap gates are not optional here:**
- Gate 5, accessibility: the study screen states the check performed and the result.
- Gate 6, Indonesian end to end: an Indonesian source generates Indonesian cards and
  each card's `label` resolves to a real chunk of that Indonesian source. This is
  the gate that catches an `{{output_language}}` default or a label-map mistake,
  both of which fail *silently* for Indonesian while English tests stay green.

---

## Global Constraints (copied from the roadmap; every task includes these)

- **No `unwrap()` in non-test code.** Typed errors mapped to HTTP status codes.
  Prompts live in versioned modules, never inline. `tracing` structured logs with
  request ids; log latency for retrieval, LLM calls, sync pull/push, jobs, webhooks.
- **Server is the only source of truth.** Clients never call LLM directly. All
  heavy work is server-side. Generation is async with visible progress.
- **Offline writes are limited to:** tasks, events, flashcard reviews, quiz
  attempts, study sessions, settings. Everything else queues nothing.
- **All times:** stored `timestamptz` UTC + user IANA timezone (default
  `Asia/Jakarta`). Local-date math only through `chrono-tz`.
- **Credits (from `config/plans.toml`, never a literal):** `flashcard_batch_10 = 4`.
  Plan limits: free `flashcards_per_month = 30`, pro and power `unlimited`.
- **Entitlements:** every handler calls `check`/the Phase 1 shim; no limit is
  hardcoded. Errors serialize to `{ code, message, min_plan?, limit?, used? }`.
- **Quality gate:** `cargo fmt --check`, `cargo clippy --all-targets -- -D warnings`,
  `cargo test`, `npm --prefix app test`, `npm --prefix app run build`, the `src-tauri` trio,
  `cargo sqlx prepare --workspace --check`.
- **UI language English**, but Indonesian documents and queries must work end to end
  (`simple` tsvector config + multilingual embeddings).

---

## What already exists (checked in this tree, not assumed)

- `migrations/0009_decks_flashcards_reviews.sql` already has `decks`, `flashcards`
  (`ease real not null default 2.5`, `interval_days int not null default 0`,
  `due_at timestamptz not null default now()`, `chunk_id uuid`) and
  `flashcard_reviews` (append-only, `id` client-supplied, `rating text`, `device_id`).
  **No schema change is needed for the review path.**
- `crates/core/src/citations.rs`: `ContextLabels::{new,chunk,chunk_count}`,
  `LabeledChunk{label,chunk_id,source_id,page}`, `Citation`, `resolve`,
  `strip_unknown`. The label map the generator needs already exists.
- `crates/core/src/prompts/mod.rs`: `compose_chat`, `ChatVars`, `PromptChunk`,
  `Composed{text,labels,prompt_version}`, `VarKind{Caller,Derived,Optional}`,
  `render`, `values_of`, `render_sources`, `escape_if_untrusted`.
- `crates/server/src/credits.rs`: `spend(state,user_id,plan,action,credits,daily_cap)
  -> Spent{usage_event_id,remaining}`, `grant_monthly`, `record_provider_usage`,
  `monthly_allowance`. `credit_ledger.reason` already documents `refund`.
- `crates/server/src/jobs/`: `enqueue`, `claim` (sets `attempts = attempts + 1`, so
  `job.attempts` is 1-based), `mark_failed` (final when `!retryable || attempts >=
  MAX_ATTEMPTS`), `Failure::{transient,permanent}`, worker `dispatch` by `kind`.
- `crates/server/src/limits.rs`: `enforce_count`, `enforce_max_file_mb`,
  `enforce_storage_mb`, all returning the `AppError::Entitlement` shape.
- `crates/core/src/traits/testing.rs`: `ScriptedLlm::new(Vec<String>)`,
  `ScriptedLlm::failing(&str)`, and **`requests() -> Vec<ChatRequest>`**, which is
  how a test proves what the prompt actually contained.
- `app/src/features/chat/ChatScreen.tsx` has a private `SourcePicker`; the SPA has
  `useSources`, `useSourceStatus(poll)`, `States.tsx`, `Field.tsx`, `Citation.tsx`.
- **MISSING:** `crates/core/src/sm2.rs` (referenced by migration 0009's comment),
  `crates/core/src/generation/`, any strict-JSON task composition, any
  `decks`/`flashcards` server module or route, any `generate_flashcards` handler,
  and any enforcement of `flashcards_per_month`.

---

## Decisions this plan pins (so parallel work cannot diverge)

1. **SM-2 replay is a pure function in `crates/core/src/sm2.rs`**, and the card's
   `ease`/`interval_days`/`due_at` are recomputed from the card's **entire** review
   history on every review, starting from the schema's initial state
   (`ease = 2.5`, `interval_days = 0`, `repetitions = 0`). Never incrementally from
   the stored values: an incremental update makes the result depend on which
   reviews that device happened to have seen, which is the bug the append-only
   design exists to prevent.
2. **The grade map and the interval rule are pinned exactly** (they are the
   algorithm's contract, not a tuning knob):
   `again = 0`, `hard = 3`, `good = 4`, `easy = 5`.
   `quality < 3` → `repetitions = 0`, `interval_days = 1`.
   Else `repetitions += 1`; `interval_days = 1` for repetition 1, `6` for
   repetition 2, `round(interval_days * ease).max(1)` after that.
   `ease = (ease + 0.1 - (5-q) * (0.08 + (5-q) * 0.02)).max(1.3)`, applied on every
   review including a failure. `due_at = reviewed_at + interval_days days`.
3. **The strict-JSON task composition lands in this phase, not Phase 6.** Phase 4 is
   the first phase that generates structured output, and the roadmap forbids inline
   prompt text. `crates/core/src/prompts/` gains `ModuleId`, the section-allowlist
   trim, the grounding/security assertion, and `compose_task`. Phase 6 extends the
   same mechanism to the render/cheatsheet modules and owns the full module sweep in
   `crates/core/tests/prompts.rs`; this phase adds the sweep for the modules that
   exist and the mechanism every later module uses. Phase 2's plan note ("Phase 6
   adds `base_sections_for`") is superseded by the roadmap's own phase ordering.
4. **`{{output_language}}` is `VarKind::Optional` with the default
   `"the same language as the source material (do not translate)"`.** There is no
   user language setting until Phase 9 (`user_settings`), and translating a
   student's material into a different language would be a silent content bug.
   When Phase 9 adds the setting, the caller supplies the value and the default is
   no longer used; nothing else changes.
5. **Card state is written only by the server.** A review is append-only and
   client-suppliable; `flashcards.ease/interval_days/due_at` are rejected on
   `POST /sync/push` (roadmap § Conflict rules, `docs/THREAT_MODEL.md` §5) and are
   never read from a request body here.
6. **Reviews are idempotent by review id.** The client may supply `id`; a duplicate
   is `on conflict (id) do nothing` and the response reports `applied: false` with
   the current state. This is what makes a retry after a lost response safe, and it
   is the same rule Phase 8's push replay relies on.
7. **`POST /flashcards/:id/review` takes `pg_advisory_xact_lock(hashtext(user_id))`
   as the first statement of its transaction.** Two devices reviewing the same card
   concurrently would otherwise each replay only their own review and the second
   `update` would win, silently dropping the other. The lock serialises the
   read-replay-write, and it is the same key the spend and the sync triggers use.
8. **The generation job refunds a spend it could not deliver.** The credit is spent
   synchronously by the route (so an unaffordable request is a 402 before anything
   is written), and a job that fails for good writes a compensating `credit_ledger`
   row with `reason = 'refund'` and `ref_id = deck_id`, once, guarded by zeroing
   `decks.credits_spent` in the same transaction. "Failed for good" means a
   `Failure::permanent`, or `job.attempts >= MAX_ATTEMPTS` on a transient one: the
   worker's give-up path is the only place that knows a retry is no longer coming.
9. **`GET /workspaces/:id/decks` is added beyond the spec's route list.** The spec
   lists `POST /workspaces/:id/decks`, `GET /decks/:id`, `GET /decks/:id/due`,
   `POST /flashcards/:id/review`. Without a list route a deck is unreachable after a
   page reload, which makes the phase's own exit gate untestable by hand. This is
   the same deliberate addition Phase 2 made for `GET /workspaces/:id/messages`, and
   it is recorded in `docs/API.md` and `docs/ASSUMPTIONS.md`.
10. **Client-side SM-2 replay is deferred to Phase 8, and client-side *due*
    computation is not.** The SPA computes the due queue from the cards it holds
    (`due_at <= now`, ordered `due_at` then `id`) in `app/src/features/study/due.ts`.
    A second SM-2 implementation in TypeScript has no consumer in this phase — the
    review is sent to the server, which returns the authoritative state — and an
    unreachable second implementation of a scheduling algorithm is how two devices
    start disagreeing. Phase 8 adds the SQLite cache and the offline outbox, and the
    replay goes with it, with vectors taken from `crates/core/tests/sm2.rs`.
11. **The generation prompt is composed from the deck's sources in `idx` order**, not
    by hybrid retrieval: there is no query to retrieve against. `GENERATION_CHUNK_LIMIT
    = 40` chunks, ordered by `(source_id, idx)`, so the same deck always gets the same
    prompt.
12. **`temperature = 0.2` and `json_mode = true`** for generation. Low because cards
    must be faithful to the material; not zero because a fully deterministic model
    produces near-duplicate cards.
13. **`MAX_REQUESTED_CARDS = 50`.** A larger request is a 400 `validation`, not a
    silent truncation: the credit cost is quoted from the requested count, so
    accepting 200 and generating 50 would overcharge.

---

## File structure (new this phase)

```
crates/core/src/
  sm2.rs                                # SM-2 replay, pure
  generation/{mod.rs,flashcards.rs}      # strict-JSON contract, parse, label resolution
  prompts/{mod.rs,registry.rs}           # + ModuleId, trim_base, compose_task
crates/core/tests/{sm2.rs,generation_flashcards.rs,prompts.rs}
crates/server/src/
  flashcards/{mod.rs,routes.rs,generate.rs}
  jobs/handlers/generate.rs
crates/server/tests/flashcards.rs
migrations/0021_flashcard_generation.sql
app/src/
  api/queries/flashcards.ts
  features/study/{StudyScreen.tsx,due.ts,due.test.ts}
  features/studio/FlashcardsPanel.tsx
  features/workspaces/SourcePicker.tsx    # extracted from ChatScreen
scripts/smoke-phase4.sh
```

**Migration numbering.** Phase 3 (a parallel session, landing first) claims
`0020_search_cache.sql`. If that file exists when this branch merges, this phase's
migration renames to `0021_flashcard_generation.sql`; sqlx rejects two files with the
same version, so the rename is required, not cosmetic. Nothing else changes.

---

## Task 1: `crates/core/src/sm2.rs` — deterministic replay

**Files:**
- Create: `crates/core/src/sm2.rs`
- Modify: `crates/core/src/lib.rs` (add `pub mod sm2;`)
- Test: `crates/core/tests/sm2.rs`

**Interfaces:**
- Produces: `Rating`, `Review`, `CardState`, `replay`, `CardState::{new,is_due}`,
  `DEFAULT_EASE`, `MIN_EASE`, `FIRST_INTERVAL_DAYS`, `SECOND_INTERVAL_DAYS`.
  Consumed by Task 5 (server replay) and Task 7's vector note.

- [ ] **Step 1: Write the failing test**

```rust
//! SM-2 replay: the contract two devices have to agree on.
//!
//! Every test here is written so that a change to the interval rule or the ease
//! formula fails loudly. These vectors are also the reference Phase 8's TypeScript
//! replay will be checked against, so the numbers are asserted exactly rather than
//! "roughly".

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use chrono::{DateTime, Utc};
use stvdy_core::sm2::{CardState, Rating, Review, DEFAULT_EASE, MIN_EASE, replay};
use uuid::Uuid;

fn at(iso: &str) -> DateTime<Utc> {
    iso.parse().expect("a valid timestamp")
}

/// A review with a deterministic id, so ordering is a property of the test and
/// not of a random uuid.
fn review(id: u128, rating: Rating, reviewed_at: &str) -> Review {
    Review {
        id: Uuid::from_u128(id),
        rating,
        reviewed_at: at(reviewed_at),
    }
}

fn ease_of(state: CardState) -> f32 {
    state.ease
}

#[test]
fn a_card_with_no_reviews_is_due_when_it_was_created() {
    let created = at("2026-10-11T00:00:00Z");
    let state = CardState::new(created);
    assert_eq!(state.ease, DEFAULT_EASE);
    assert_eq!(state.interval_days, 0);
    assert_eq!(state.due_at, created);
    assert!(state.is_due(created));
}

#[test]
fn three_good_reviews_give_intervals_one_six_and_fifteen() {
    let created = at("2026-10-11T00:00:00Z");
    let state = replay(
        created,
        &[
            review(1, Rating::Good, "2026-10-11T09:00:00Z"),
            review(2, Rating::Good, "2026-10-12T09:00:00Z"),
            review(3, Rating::Good, "2026-10-18T09:00:00Z"),
        ],
    );
    assert_eq!(state.interval_days, 15);
    // 6 * 2.5 = 15, and `good` leaves ease exactly where it was.
    assert!((ease_of(state) - 2.5).abs() < 1e-6, "{}", ease_of(state));
    assert_eq!(state.due_at, at("2026-11-02T09:00:00Z"));
}

#[test]
fn easy_raises_ease_and_hard_lowers_it() {
    let created = at("2026-10-11T00:00:00Z");
    let easy = replay(created, &[review(1, Rating::Easy, "2026-10-11T09:00:00Z")]);
    assert_eq!(easy.interval_days, 1);
    assert!((ease_of(easy) - 2.6).abs() < 1e-6, "{}", ease_of(easy));

    let hard = replay(created, &[review(1, Rating::Hard, "2026-10-11T09:00:00Z")]);
    assert_eq!(hard.interval_days, 1);
    assert!((ease_of(hard) - 2.36).abs() < 1e-6, "{}", ease_of(hard));
}

#[test]
fn again_resets_the_interval_and_drops_ease_but_not_below_the_floor() {
    let created = at("2026-10-11T00:00:00Z");
    let once = replay(
        created,
        &[
            review(1, Rating::Good, "2026-10-11T09:00:00Z"),
            review(2, Rating::Good, "2026-10-12T09:00:00Z"),
            review(3, Rating::Again, "2026-10-18T09:00:00Z"),
        ],
    );
    // The interval resets, and the ease after two `good`s is still 2.5.
    assert_eq!(once.interval_days, 1);
    assert!((ease_of(once) - 1.7).abs() < 1e-6, "{}", ease_of(once));
    assert_eq!(once.due_at, at("2026-10-19T09:00:00Z"));

    let floored = replay(
        created,
        &[
            review(1, Rating::Again, "2026-10-11T09:00:00Z"),
            review(2, Rating::Again, "2026-10-12T09:00:00Z"),
            review(3, Rating::Again, "2026-10-13T09:00:00Z"),
        ],
    );
    assert_eq!(ease_of(floored), MIN_EASE);
}

/// The property the whole design rests on: the review rows are the facts and the
/// card state is a function of them, so arrival order cannot matter.
#[test]
fn the_arrival_order_of_reviews_does_not_change_the_state() {
    let created = at("2026-10-11T00:00:00Z");
    let forwards = vec![
        review(1, Rating::Good, "2026-10-11T09:00:00Z"),
        review(2, Rating::Again, "2026-10-12T09:00:00Z"),
        review(3, Rating::Easy, "2026-10-13T09:00:00Z"),
    ];
    let mut backwards = forwards.clone();
    backwards.reverse();
    assert_eq!(replay(created, &forwards), replay(created, &backwards));

    // And the state is the one the rules produce, written out rather than compared
    // with another call: `replay(x) == replay(x)` can only fail if `replay` grows a
    // clock or a random source, so it pins nothing about the schedule.
    let state = replay(created, &forwards);
    assert_eq!(state.interval_days, 1);
    assert_eq!(state.due_at, at("2026-10-14T09:00:00Z"));
    assert!((ease_of(state) - 1.8).abs() < 1e-6, "{}", ease_of(state));
}

/// Two reviews that share a timestamp are ordered by id, so a device that
/// reviewed twice in the same second still replays deterministically.
#[test]
fn reviews_at_the_same_instant_are_ordered_by_id() {
    let created = at("2026-10-11T00:00:00Z");
    let low = review(1, Rating::Good, "2026-10-11T09:00:00Z");
    let high = review(2, Rating::Easy, "2026-10-11T09:00:00Z");
    assert_eq!(
        replay(created, &[low.clone(), high.clone()]),
        replay(created, &[high, low])
    );
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p stvdy-core --test sm2`
Expected: FAIL, `unresolved import stvdy_core::sm2`.

- [ ] **Step 3: Write the implementation**

```rust
//! SM-2 spaced repetition, as a deterministic replay.
//!
//! `flashcards.ease`, `interval_days` and `due_at` are **derived**: they are never
//! authored by a client (the sync conflict table and `docs/THREAT_MODEL.md` §5 both
//! say so). They are recomputed here by replaying the card's `flashcard_reviews`
//! rows in `(reviewed_at, id)` order from the schema's initial state. That is what
//! makes reviews from several devices conflict-free: the reviews are append-only
//! facts and the card state is a pure function of them, so any arrival order
//! produces one state. An incremental update from the stored values would instead
//! make the result depend on which reviews that device had seen, which is the bug
//! the append-only design exists to prevent.
//!
//! No clock, no IO, no database. `reviewed_at` comes from the review row, never
//! from `Utc::now()`, so a replay is reproducible from stored data alone.

use chrono::{DateTime, Duration, Utc};
use uuid::Uuid;

use crate::error::CoreError;

/// The ease factor a card starts with. Matches `flashcards.ease`'s default.
pub const DEFAULT_EASE: f32 = 2.5;
/// The floor SM-2 puts on ease, so a card failed repeatedly still becomes
/// reviewable instead of sitting at a one-day interval forever.
pub const MIN_EASE: f32 = 1.3;
/// The interval after the first successful review, in days.
pub const FIRST_INTERVAL_DAYS: i32 = 1;
/// The interval after the second successful review, in days.
pub const SECOND_INTERVAL_DAYS: i32 = 6;

/// The four grades the study screen offers, matching
/// `flashcard_reviews.rating`'s documented values.
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Rating {
    Again,
    Hard,
    Good,
    Easy,
}

impl Rating {
    pub fn as_str(self) -> &'static str {
        match self {
            Rating::Again => "again",
            Rating::Hard => "hard",
            Rating::Good => "good",
            Rating::Easy => "easy",
        }
    }

    /// Parses the wire form. Rejecting an unknown grade is deliberate: a typo
    /// silently treated as `good` would move a card's schedule on bad input.
    pub fn parse(raw: &str) -> Result<Rating, CoreError> {
        match raw {
            "again" => Ok(Rating::Again),
            "hard" => Ok(Rating::Hard),
            "good" => Ok(Rating::Good),
            "easy" => Ok(Rating::Easy),
            other => Err(CoreError::validation(
                "rating",
                format!("unknown review rating {other:?} (expected again, hard, good or easy)"),
            )),
        }
    }

    /// The SM-2 quality grade. `again` is below the pass threshold of 3.
    pub fn quality(self) -> i32 {
        match self {
            Rating::Again => 0,
            Rating::Hard => 3,
            Rating::Good => 4,
            Rating::Easy => 5,
        }
    }
}

/// One `flashcard_reviews` row, as the replay sees it.
#[derive(Debug, Clone, PartialEq)]
pub struct Review {
    pub id: Uuid,
    pub rating: Rating,
    pub reviewed_at: DateTime<Utc>,
}

/// A card's derived scheduling state.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct CardState {
    pub ease: f32,
    pub interval_days: i32,
    pub due_at: DateTime<Utc>,
}

impl CardState {
    /// The state of a card that has never been reviewed: due immediately.
    pub fn new(created_at: DateTime<Utc>) -> Self {
        Self {
            ease: DEFAULT_EASE,
            interval_days: 0,
            due_at: created_at,
        }
    }

    /// Whether the card is in the study queue at `now`.
    pub fn is_due(&self, now: DateTime<Utc>) -> bool {
        self.due_at <= now
    }
}

/// Replays `reviews` onto a card created at `created_at`.
///
/// Sorts a copy of the slice, so the caller's order is irrelevant and two devices
/// that received the reviews in different orders agree.
pub fn replay(created_at: DateTime<Utc>, reviews: &[Review]) -> CardState {
    let mut ordered: Vec<&Review> = reviews.iter().collect();
    ordered.sort_by(|left, right| {
        left.reviewed_at
            .cmp(&right.reviewed_at)
            .then_with(|| left.id.cmp(&right.id))
    });

    let mut state = CardState::new(created_at);
    let mut repetitions: i32 = 0;

    for review in ordered {
        let quality = review.rating.quality();

        if quality < 3 {
            // A failure sends the card back to the start of the ladder rather than
            // to zero: it is due again tomorrow, which is what "relearn" means.
            repetitions = 0;
            state.interval_days = FIRST_INTERVAL_DAYS;
        } else {
            repetitions += 1;
            state.interval_days = match repetitions {
                1 => FIRST_INTERVAL_DAYS,
                2 => SECOND_INTERVAL_DAYS,
                _ => ((state.interval_days as f32) * state.ease).round().max(1.0) as i32,
            };
        }

        // Applied on every review, including a failure: SM-2 lowers ease on a
        // lapse, and a card that never loses ease would grow without bound.
        let q = quality as f32;
        state.ease = (state.ease + 0.1 - (5.0 - q) * (0.08 + (5.0 - q) * 0.02)).max(MIN_EASE);

        state.due_at = review.reviewed_at + Duration::days(state.interval_days as i64);
    }

    state
}

#[cfg(test)]
#[allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]
mod tests {
    use super::*;

    #[test]
    fn an_unknown_rating_is_rejected_rather_than_defaulted() {
        assert!(Rating::parse("Good").is_err());
        assert!(Rating::parse("").is_err());
        assert_eq!(Rating::parse("again").unwrap(), Rating::Again);
    }

    #[test]
    fn every_rating_round_trips_through_its_wire_form() {
        for rating in [Rating::Again, Rating::Hard, Rating::Good, Rating::Easy] {
            assert_eq!(Rating::parse(rating.as_str()).unwrap(), rating);
        }
    }
}
```

Add to `crates/core/src/lib.rs`, keeping the alphabetical order (only `sm2` — the
`generation` module arrives in Task 3, and declaring it here would not compile):

```rust
pub mod sm2;
```

`CoreError::validation(field, reason)` already exists (`crates/core/src/error.rs:34`);
it takes **two** arguments, and the brief's call sites use `"rating"` as the field.

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p stvdy-core --test sm2 && cargo test -p stvdy-core sm2`
Expected: PASS (6 integration tests + 2 unit tests).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/sm2.rs crates/core/src/lib.rs crates/core/tests/sm2.rs
git commit -m "feat(core): deterministic SM-2 replay, the one schedule two devices share"
```

---

## Task 2: `crates/core/src/prompts/` — strict-JSON task composition

**Files:**
- Modify: `crates/core/src/prompts/registry.rs`, `crates/core/src/prompts/mod.rs`
- Test: `crates/core/tests/prompts.rs` (extend)

**Interfaces:**
- Consumes: `VarKind`, `render`, `values_of`, `render_sources`, `ChatVars`,
  `PromptChunk`, `Composed` (all existing).
- Produces: `registry::ModuleId`, `registry::{FLASHCARDS_V1, JSON_CONTRACT_V1,
  TASK_BASE_SECTIONS, SOURCE_LANGUAGE}`, `prompts::{compose_task, trim_base,
  module_variables}`, `PromptError::{UndeclaredVariable, MissingSection}`.
  Consumed by Task 4 (the generation job) and Task 3 (its tests).

- [ ] **Step 1: Write the failing test**

Append to `crates/core/tests/prompts.rs`:

```rust
/// The task-composition contract, asserted for every module that exists.
///
/// The roadmap's rule is that a task prompt must never lose its grounding or its
/// injection defences, and that an unfilled variable must never reach a model.
/// Both are checked here rather than trusted to the composer's implementation.
mod task_composition {
    use stvdy_core::prompts::{
        ChatVars, ModuleId, PromptChunk, PromptError, compose_task, module_variables, trim_base,
    };
    use std::collections::BTreeMap;
    use uuid::Uuid;

    fn base() -> ChatVars {
        ChatVars {
            assistant_name: "Stvdy".to_string(),
            app_name: "Stvdy".to_string(),
            now: "2026-10-11T09:00:00+07:00".to_string(),
            timezone: "Asia/Jakarta".to_string(),
            platform: "web".to_string(),
            plan_name: "Free".to_string(),
            study_mode: "off".to_string(),
            study_hours_summary: "19:00-22:00".to_string(),
            use_web: "off".to_string(),
            allow_upgrade_hint: false,
            user_name: "Jetta".to_string(),
            workspace_name: "Jaringan Komputer".to_string(),
        }
    }

    fn chunk(label: u32) -> PromptChunk {
        PromptChunk {
            label,
            chunk_id: Uuid::from_u128(u128::from(label)),
            source_id: Uuid::from_u128(99),
            source_title: "Modul 3".to_string(),
            page: Some(4),
            content: "TCP memakai three-way handshake untuk membuka koneksi.".to_string(),
        }
    }

    /// Every `{{name}}` in a template, in first-appearance order.
    fn variables_in(text: &str) -> Vec<String> {
        let mut found = Vec::new();
        let mut rest = text;
        while let Some(open) = rest.find("{{") {
            let after = &rest[open + 2..];
            let Some(close) = after.find("}}") else { break };
            let name = after[..close].to_string();
            if !found.contains(&name) {
                found.push(name);
            }
            rest = &after[close + 2..];
        }
        found
    }

    #[test]
    fn every_module_declares_exactly_the_variables_its_file_uses() {
        for module in ModuleId::ALL {
            let declared: Vec<&str> = module_variables(module)
                .iter()
                .map(|(name, _)| *name)
                .collect();
            let mut used = variables_in(module.text());
            // The module is rendered together with the JSON contract, so a
            // variable used only there is still the module's to declare.
            for name in variables_in(stvdy_core::prompts::json_contract()) {
                if !used.contains(&name) {
                    used.push(name);
                }
            }
            for name in &used {
                assert!(
                    declared.contains(&name.as_str()),
                    "{} uses {{{{{name}}}}} but does not declare it",
                    module.version()
                );
            }
            for name in &declared {
                assert!(
                    used.iter().any(|u| u == name),
                    "{} declares {{{{{name}}}}} but its files never use it",
                    module.version()
                );
            }
        }
    }

    #[test]
    fn a_derived_variable_is_never_declared_as_caller_supplied_in_a_module() {
        for module in ModuleId::ALL {
            for (name, kind) in module_variables(module) {
                assert!(
                    !matches!(kind, stvdy_core::prompts::VarKind::Derived),
                    "{} declares {name} as derived; derived variables belong to the base",
                    module.version()
                );
            }
        }
    }

    #[test]
    fn the_trimmed_base_keeps_grounding_and_security_and_drops_the_prose_sections() {
        let trimmed = trim_base(ModuleId::FlashcardsV1).unwrap();
        assert!(trimmed.contains("<grounding_and_citations>"), "{trimmed}");
        assert!(trimmed.contains("<security_untrusted_content>"), "{trimmed}");
        assert!(trimmed.contains("<user_context_data>"), "{trimmed}");
        for dropped in ["<teaching_style>", "<tools_and_actions>", "<planning_help>"] {
            assert!(!trimmed.contains(dropped), "{dropped} survived the trim");
        }
    }

    #[test]
    fn trimming_away_a_required_section_is_an_error() {
        // The guard rail, exercised directly: an allowlist without grounding must
        // fail rather than produce a task prompt that invites uncited invention.
        let error = stvdy_core::prompts::trim_base_with(
            stvdy_core::prompts::base_v1(),
            &["context"],
        )
        .unwrap_err();
        assert!(matches!(error, PromptError::MissingSection(_)), "{error:?}");
    }

    #[test]
    fn compose_task_fills_the_module_variables_and_leaves_no_marker() {
        let mut module_vars = BTreeMap::new();
        module_vars.insert("count", "10".to_string());
        let composed =
            compose_task(ModuleId::FlashcardsV1, &base(), &module_vars, &[chunk(1)]).unwrap();
        assert!(!composed.text.contains("{{"), "{}", composed.text);
        assert!(composed.text.contains("10 cards requested"), "{}", composed.text);
        assert_eq!(composed.prompt_version, "flashcards.v1");
        // The label map is what turns the model's `label` into a real chunk id.
        assert_eq!(composed.labels.chunk(1).unwrap().chunk_id, Uuid::from_u128(1));
    }

    #[test]
    fn an_undeclared_module_variable_is_refused() {
        let mut module_vars = BTreeMap::new();
        module_vars.insert("count", "10".to_string());
        module_vars.insert("typo", "10".to_string());
        let error =
            compose_task(ModuleId::FlashcardsV1, &base(), &module_vars, &[chunk(1)]).unwrap_err();
        assert!(matches!(error, PromptError::UndeclaredVariable(name) if name == "typo"));
    }

    #[test]
    fn a_missing_required_module_variable_is_an_error_not_an_empty_string() {
        let module_vars = BTreeMap::new();
        let error =
            compose_task(ModuleId::FlashcardsV1, &base(), &module_vars, &[chunk(1)]).unwrap_err();
        assert!(matches!(error, PromptError::MissingVariable(name) if name == "count"));
    }

    /// The default is what makes Indonesian work without a language setting: the
    /// cards come out in the material's language.
    #[test]
    fn output_language_defaults_to_the_material_language() {
        let mut module_vars = BTreeMap::new();
        module_vars.insert("count", "3".to_string());
        let composed =
            compose_task(ModuleId::FlashcardsV1, &base(), &module_vars, &[chunk(1)]).unwrap();
        assert!(
            composed.text.contains("the same language as the source material"),
            "{}",
            composed.text
        );
    }
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p stvdy-core --test prompts`
Expected: FAIL, `no ModuleId in stvdy_core::prompts`.

- [ ] **Step 3: Extend the registry**

Replace the body of `crates/core/src/prompts/registry.rs` with (keeping the existing
two consts and their doc comments):

```rust
//! Compiled-in prompt modules.
//!
//! `include_str!` means a missing or renamed prompt file is a build error, not a
//! runtime 500 in front of a user. The version string is the filename stem, and it
//! is what lands in `usage_events.prompt_version` so a quality regression can be
//! attributed to a prompt change rather than guessed at.

use super::VarKind;

/// The chat base prompt. Every chat and assistant turn is composed on top of
/// this text, never on inline prompt strings scattered through the server.
pub const BASE_V1: &str = include_str!("../../../../prompts/base.v1.md");

/// The version stored with every request composed from [`BASE_V1`].
pub const BASE_V1_VERSION: &str = "base.v1";

/// The shared strict-JSON preamble every task module is followed by.
pub const JSON_CONTRACT_V1: &str = include_str!("../../../../prompts/json_contract.v1.md");

/// The flashcards module.
pub const FLASHCARDS_V1: &str = include_str!("../../../../prompts/flashcards.v1.md");

/// What `{{output_language}}` means when the caller supplies nothing.
///
/// A study app's cards must be in the language of the student's material. There is
/// no language setting until Phase 9, and translating a deck into English would be
/// a silent content bug for every Indonesian student, so the default names the
/// material rather than picking a language.
pub const SOURCE_LANGUAGE: &str = "the same language as the source material (do not translate)";

/// The `base.v1.md` sections a strict-JSON task keeps.
///
/// The three dropped by name in `prompts/README.md` (`teaching_style`,
/// `tools_and_actions`, `planning_help`) all invite prose in a call that must
/// return one JSON object. `plans_and_billing` and `when_things_go_wrong` are
/// dropped for the same reason: both are instructions about what to *say* to the
/// user, and there is no user-facing turn here. The safety, integrity, privacy and
/// language sections stay: a source document can contain anything, and the model
/// must still refuse to reproduce what it must not.
pub const TASK_BASE_SECTIONS: &[&str] = &[
    "context",
    "user_context_data",
    "language_and_tone",
    "grounding_and_citations",
    "security_untrusted_content",
    "academic_integrity",
    "wellbeing_and_safety",
    "privacy_and_identity",
];

/// A strict-JSON task module: its text, its variables, and the base sections it
/// keeps. One variant per file under `prompts/`, added here and in [`ModuleId::ALL`]
/// when a phase adds a module.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ModuleId {
    FlashcardsV1,
}

impl ModuleId {
    /// Every module, for the contract sweep in `tests/prompts.rs`.
    pub const ALL: [ModuleId; 1] = [ModuleId::FlashcardsV1];

    /// The version string logged to `usage_events.prompt_version`.
    pub fn version(self) -> &'static str {
        match self {
            ModuleId::FlashcardsV1 => "flashcards.v1",
        }
    }

    pub fn text(self) -> &'static str {
        match self {
            ModuleId::FlashcardsV1 => FLASHCARDS_V1,
        }
    }

    /// The variables the module's own text (and the JSON contract) uses.
    pub fn variables(self) -> &'static [(&'static str, VarKind)] {
        match self {
            ModuleId::FlashcardsV1 => &[
                ("count", VarKind::Caller),
                ("output_language", VarKind::Optional(SOURCE_LANGUAGE)),
            ],
        }
    }

    /// The `base.v1.md` sections this module keeps.
    pub fn base_sections(self) -> &'static [&'static str] {
        match self {
            ModuleId::FlashcardsV1 => TASK_BASE_SECTIONS,
        }
    }
}
```

- [ ] **Step 4: Extend the composer**

In `crates/core/src/prompts/mod.rs`:

1. Add `pub mod registry;` is already there; add `pub use registry::ModuleId;` next to it.

2. Add the two error variants to `PromptError`:

```rust
    #[error("prompt module uses {{{0}}} without declaring it")]
    UndeclaredVariable(String),
    #[error("the trimmed base prompt lost <{0}>, which every task prompt must keep")]
    MissingSection(String),
```

3. Generalise the renderer. `render` currently reads its variable table from
   `base_variable_kind`, which knows only the base. Split that out and keep the
   existing signature as a thin wrapper, so the chat path is untouched:

```rust
/// The declared kind of a variable in `table`.
fn variable_kind(table: &[(&'static str, VarKind)], name: &str) -> Option<VarKind> {
    table
        .iter()
        .find(|(declared, _)| *declared == name)
        .map(|(_, kind)| *kind)
}

fn render(template: &str, values: &BTreeMap<&'static str, String>) -> Result<String, PromptError> {
    render_with(template, values, base_variables())
}

/// Fills a template's variables from an explicit declaration table.
///
/// `render` uses the base's table; a task module passes its own plus the base's, so
/// `{{count}}` resolves for the module and `{{now}}` still resolves for the trimmed
/// base. Derived variables are computed, never read, and a caller that supplies one
/// is refused outright.
fn render_with(
    template: &str,
    values: &BTreeMap<&'static str, String>,
    table: &[(&'static str, VarKind)],
) -> Result<String, PromptError> {
    for name in values.keys() {
        if matches!(variable_kind(table, name), Some(VarKind::Derived)) {
            return Err(PromptError::DerivedSupplied((*name).to_string()));
        }
    }

    let mut output = String::with_capacity(template.len() + 1024);
    let mut rest = template;

    while let Some(open) = rest.find("{{") {
        output.push_str(&rest[..open]);
        let after_open = &rest[open + 2..];
        let Some(close) = after_open.find("}}") else {
            output.push_str(&rest[open..]);
            return Ok(output);
        };
        let name = &after_open[..close];
        rest = &after_open[close + 2..];

        match variable_kind(table, name) {
            Some(VarKind::Derived) => output.push_str(&derived_value(name, values)?),
            Some(VarKind::Caller) => {
                let value = values
                    .get(name)
                    .filter(|value| !value.is_empty())
                    .ok_or_else(|| PromptError::MissingVariable(name.to_string()))?;
                output.push_str(&escape_if_untrusted(name, value));
            }
            Some(VarKind::Optional(default)) => match values.get(name) {
                Some(value) if !value.is_empty() => output.push_str(value),
                _ => output.push_str(default),
            },
            None => {
                output.push_str("{{");
                output.push_str(name);
                output.push_str("}}");
            }
        }
    }
    output.push_str(rest);
    Ok(output)
}
```

4. Extract the label map, so both composers build it one way:

```rust
/// The per-request `label -> chunk` map, built the same way for chat and for tasks.
fn labels_of(chunks: &[PromptChunk], web: &[PromptWebHit]) -> ContextLabels {
    ContextLabels::new(
        chunks
            .iter()
            .map(|chunk| LabeledChunk {
                label: chunk.label,
                chunk_id: chunk.chunk_id,
                source_id: chunk.source_id,
                page: chunk.page,
            })
            .collect(),
        web.iter()
            .map(|hit| LabeledWeb {
                label: hit.label,
                url: hit.url.clone(),
                title: hit.title.clone(),
            })
            .collect(),
    )
}
```

and replace the inline construction at the end of `compose_chat` with
`let labels = labels_of(chunks, web);`.

5. Add the public surface the task path needs:

```rust
/// The base prompt text, for the contract tests and for `trim_base`.
pub fn base_v1() -> &'static str {
    registry::BASE_V1
}

/// The JSON contract text, so a test can assert the module's declared variables
/// cover it too.
pub fn json_contract() -> &'static str {
    registry::JSON_CONTRACT_V1
}

/// A module's declared variables.
pub fn module_variables(module: ModuleId) -> &'static [(&'static str, VarKind)] {
    module.variables()
}

/// The base prompt with every section not in `module`'s allowlist removed.
///
/// Public because the contract test asserts the guard rail directly: trimming away
/// `<grounding_and_citations>` or `<security_untrusted_content>` must fail, not
/// produce a task prompt that invites uncited invention.
pub fn trim_base(module: ModuleId) -> Result<String, PromptError> {
    trim_base_with(registry::BASE_V1, module.base_sections())
}

/// [`trim_base`] against an explicit allowlist.
pub fn trim_base_with(template: &str, keep: &[&str]) -> Result<String, PromptError> {
    let mut output = String::with_capacity(template.len());
    let mut dropping = false;

    for line in template.lines() {
        let trimmed = line.trim();
        if let Some(name) = opening_section(trimmed) {
            dropping = !keep.contains(&name);
            if !dropping {
                output.push_str(line);
                output.push('\n');
            }
            continue;
        }
        if closing_section(trimmed).is_some() {
            if !dropping {
                output.push_str(line);
                output.push('\n');
            }
            dropping = false;
            continue;
        }
        if !dropping {
            output.push_str(line);
            output.push('\n');
        }
    }

    // The guard rail `prompts/README.md` states: a task prompt must never be
    // composed without its grounding and untrusted-content rules.
    for required in ["grounding_and_citations", "security_untrusted_content"] {
        if !output.contains(&format!("<{required}>")) {
            return Err(PromptError::MissingSection(required.to_string()));
        }
    }

    Ok(output.trim_end().to_string())
}

/// `<name>` on its own line, with no attributes and no closing slash.
fn opening_section(line: &str) -> Option<&str> {
    let inner = line.strip_prefix('<')?.strip_suffix('>')?;
    if inner.is_empty() || inner.starts_with('/') || inner.contains(['<', ' ', '\t']) {
        return None;
    }
    Some(inner)
}

fn closing_section(line: &str) -> Option<&str> {
    let inner = line.strip_prefix("</")?.strip_suffix('>')?;
    if inner.is_empty() || inner.contains(['<', ' ', '\t']) {
        return None;
    }
    Some(inner)
}

/// Renders a strict-JSON task prompt: the trimmed base, the shared JSON contract,
/// the module, then the labelled sources.
///
/// The order is fixed by `prompts/README.md`: the contract must come before the
/// module so the module's own schema is the last thing the model reads, and the
/// sources must come last so the data block is never followed by instructions.
pub fn compose_task(
    module: ModuleId,
    base: &ChatVars,
    module_vars: &BTreeMap<&'static str, String>,
    chunks: &[PromptChunk],
) -> Result<Composed, PromptError> {
    let table = module.variables();

    // An undeclared name is refused here rather than left in the text: `render_with`
    // would leave `{{typo}}` intact and the no-`{{` assertion below would report it
    // as an unfilled variable, which reads as a caller mistake when it is a
    // declaration mistake.
    let mut values = values_of(base);
    for (name, value) in module_vars {
        if !table.iter().any(|(declared, _)| declared == name) {
            return Err(PromptError::UndeclaredVariable((*name).to_string()));
        }
        values.insert(name, value.clone());
    }

    let trimmed = trim_base(module)?;
    let mut text = render_with(&trimmed, &values, base_variables())?;
    text.push_str("\n\n");
    text.push_str(&render_with(registry::JSON_CONTRACT_V1, &values, table)?);
    text.push_str("\n\n");
    text.push_str(&render_with(module.text(), &values, table)?);
    text.push_str("\n\n");
    text.push_str(&render_sources(chunks));

    if text.contains("{{") {
        return Err(PromptError::Invalid(
            "composed task prompt still contains an unfilled {{variable}}".to_string(),
        ));
    }

    Ok(Composed {
        text,
        labels: labels_of(chunks, &[]),
        prompt_version: module.version().to_string(),
    })
}
```

- [ ] **Step 5: Run the tests to verify they pass**

Run: `cargo test -p stvdy-core --test prompts && cargo test -p stvdy-core`
Expected: PASS. The existing chat tests must still pass unchanged: `render` and
`compose_chat` keep their signatures.

- [ ] **Step 6: Commit**

```bash
git add crates/core/src/prompts crates/core/tests/prompts.rs
git commit -m "feat(core): the strict-JSON task composer, with the grounding guard rail"
```

---

## Task 3: `crates/core/src/generation/` — the flashcards contract

**Files:**
- Create: `crates/core/src/generation/mod.rs`, `crates/core/src/generation/flashcards.rs`
- Test: `crates/core/tests/generation_flashcards.rs`

**Interfaces:**
- Consumes: `citations::ContextLabels`, `Uuid`.
- Produces: `GeneratedCard`, `ResolvedCard`, `ParseOutcome`, `GenerationError`,
  `parse_cards`, `resolve_cards`, `MAX_CARDS_PER_REPLY`.
  Consumed by Task 4 (the job).

- [ ] **Step 1: Write the failing test**

```rust
//! The flashcards JSON contract: parsing a model reply and resolving its labels.
//!
//! A provider is not available here and must not be: parsing and label resolution
//! are pure, and they are where the two defects that reach a user live (a reply
//! that is nearly JSON, and a card citing a label that was never sent).

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use stvdy_core::citations::{ContextLabels, LabeledChunk};
use stvdy_core::generation::GenerationError;
use stvdy_core::generation::flashcards::{parse_cards, resolve_cards};
use uuid::Uuid;

fn labels() -> ContextLabels {
    ContextLabels::new(
        vec![
            LabeledChunk {
                label: 1,
                chunk_id: Uuid::from_u128(11),
                source_id: Uuid::from_u128(1),
                page: Some(3),
            },
            LabeledChunk {
                label: 2,
                chunk_id: Uuid::from_u128(22),
                source_id: Uuid::from_u128(1),
                page: None,
            },
        ],
        vec![],
    )
}

#[test]
fn a_plain_object_parses() {
    let cards = parse_cards(r#"{"cards":[{"front":"Apa itu TCP?","back":"Protokol transport.","label":1}]}"#)
        .unwrap();
    assert_eq!(cards.len(), 1);
    assert_eq!(cards[0].label, 1);
}

#[test]
fn a_fenced_reply_parses_because_a_provider_sometimes_adds_one() {
    let raw = "```json\n{\"cards\":[{\"front\":\"a\",\"back\":\"b\",\"label\":1}]}\n```";
    assert_eq!(parse_cards(raw).unwrap().len(), 1);
}

#[test]
fn prose_around_the_object_is_not_guessed_at() {
    let raw = "Here are your cards:\n{\"cards\":[]}";
    // serde classifies leading prose as a syntax error, not a schema error, so this
    // is `Json` and not `Schema`. What matters for the caller is that both are
    // retryable and neither is guessed at.
    assert!(matches!(parse_cards(raw), Err(GenerationError::Json(_))));
}

#[test]
fn an_extra_field_is_rejected_rather_than_ignored() {
    let raw = r#"{"cards":[{"front":"a","back":"b","label":1,"hint":"c"}]}"#;
    assert!(matches!(parse_cards(raw), Err(GenerationError::Schema(_))));
}

#[test]
fn an_empty_deck_is_an_error_not_an_empty_success() {
    assert!(matches!(parse_cards(r#"{"cards":[]}"#), Err(GenerationError::Empty)));
}

#[test]
fn a_label_that_was_never_sent_is_dropped_and_recorded() {
    let cards = parse_cards(
        r#"{"cards":[
             {"front":"a","back":"b","label":1},
             {"front":"c","back":"d","label":99},
             {"front":"e","back":"f","label":99}
           ]}"#,
    )
    .unwrap();
    let outcome = resolve_cards(cards, &labels());
    // Three cards in, three cards out: the label costs the card its link, not its
    // life (see the exit checklist), so only `dropped_labels` records the loss.
    assert_eq!(outcome.cards.len(), 3);
    assert_eq!(outcome.cards[0].chunk_id, Some(Uuid::from_u128(11)));
    // Dropped, never rewritten to a nearby label, and counted once.
    assert_eq!(outcome.cards[1].chunk_id, None);
    assert_eq!(outcome.dropped_labels, vec![99]);
}

#[test]
fn a_card_with_an_empty_side_is_dropped() {
    let cards = parse_cards(
        r#"{"cards":[{"front":"  ","back":"b","label":1},{"front":"c","back":"","label":1}]}"#,
    )
    .unwrap();
    let outcome = resolve_cards(cards, &labels());
    assert!(outcome.cards.is_empty(), "{:?}", outcome.cards);
}
```

- [ ] **Step 2: Run the test to verify it fails**

Run: `cargo test -p stvdy-core --test generation_flashcards`
Expected: FAIL, `no generation in stvdy_core`.

- [ ] **Step 3: Write the implementation**

`crates/core/src/generation/mod.rs`:

```rust
//! Strict-JSON generation: the model's reply, turned into typed domain data.
//!
//! Every module here is a pure function of the reply text and the label map. The
//! retry-once rule lives in the caller (the server, which owns the provider), but
//! the *decision* it retries on — is this a parse failure — is made here, so the
//! rule is testable without a model.

pub mod flashcards;

#[derive(Debug, thiserror::Error)]
pub enum GenerationError {
    #[error("the model's reply was not valid JSON: {0}")]
    Json(String),
    #[error("the model's reply did not match the schema: {0}")]
    Schema(String),
    #[error("the model returned no cards")]
    Empty,
}

/// True when a second attempt is worth making.
///
/// A parse failure is worth one retry with a corrective message; an empty deck is
/// not, because the model answered the question it was asked and the answer was
/// "no cards", and retrying would spend a second call to be told the same thing.
pub fn is_retryable(error: &GenerationError) -> bool {
    matches!(error, GenerationError::Json(_) | GenerationError::Schema(_))
}
```

`crates/core/src/generation/flashcards.rs`:

```rust
//! The flashcards JSON contract (`prompts/flashcards.v1.md`), parsed and resolved.

use uuid::Uuid;

use crate::citations::ContextLabels;
use crate::generation::GenerationError;

/// The most cards one reply may carry. Above the largest request the API accepts,
/// so a model that ignores `{{count}}` cannot store an unbounded deck.
pub const MAX_CARDS_PER_REPLY: usize = 60;

/// The reply, exactly as `prompts/flashcards.v1.md` documents it.
///
/// `deny_unknown_fields` is what makes a drifted prompt fail loudly: a model that
/// invents `hint` or `difficulty` produces a parse error and a retry rather than a
/// card with a silently dropped field.
#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CardsFile {
    pub cards: Vec<GeneratedCard>,
}

#[derive(Debug, Clone, serde::Deserialize)]
#[serde(deny_unknown_fields)]
pub struct GeneratedCard {
    pub front: String,
    pub back: String,
    /// The chunk label the card came from, as shown inside `<sources>`.
    pub label: u32,
}

/// One card with its chunk resolved, or `chunk_id: None` when the label was not in
/// the context (A17: dropped, never guessed).
#[derive(Debug, Clone)]
pub struct ResolvedCard {
    pub front: String,
    pub back: String,
    pub chunk_id: Option<Uuid>,
    pub label: u32,
}

#[derive(Debug, Clone, Default)]
pub struct ParseOutcome {
    pub cards: Vec<ResolvedCard>,
    /// Labels the model cited that were not in the context, deduplicated and
    /// sorted. Recorded rather than silently swallowed: a rate that climbs means
    /// the prompt or the model changed.
    pub dropped_labels: Vec<u32>,
}

/// Parses a model reply into cards.
///
/// Tolerates a fenced code block, because a provider occasionally adds one despite
/// the contract. It does **not** tolerate prose around the object: guessing which
/// part is the JSON is how a half-parsed deck ships.
pub fn parse_cards(raw: &str) -> Result<Vec<GeneratedCard>, GenerationError> {
    let text = strip_code_fence(raw);
    let file: CardsFile = serde_json::from_str(text).map_err(|error| {
        // `serde_json` reports both "not JSON" and "wrong shape" through the same
        // error type; the message distinguishes them, and the caller only needs to
        // know a retry is worth one attempt.
        if error.is_syntax() || error.is_eof() {
            GenerationError::Json(error.to_string())
        } else {
            GenerationError::Schema(error.to_string())
        }
    })?;

    if file.cards.is_empty() {
        return Err(GenerationError::Empty);
    }
    if file.cards.len() > MAX_CARDS_PER_REPLY {
        return Err(GenerationError::Schema(format!(
            "the model returned {} cards, more than the {MAX_CARDS_PER_REPLY} allowed",
            file.cards.len()
        )));
    }
    Ok(file.cards)
}

/// ```` ```json … ``` ```` becomes `…`; anything else is returned trimmed.
fn strip_code_fence(raw: &str) -> &str {
    let trimmed = raw.trim();
    let Some(rest) = trimmed.strip_prefix("```") else {
        return trimmed;
    };
    let body = match rest.find('\n') {
        Some(index) => &rest[index + 1..],
        None => rest,
    };
    body.trim_end()
        .strip_suffix("```")
        .map(str::trim)
        .unwrap_or(body)
}

/// Resolves each card's label against the labels the prompt actually sent, and
/// drops the cards that carry nothing to store.
pub fn resolve_cards(cards: Vec<GeneratedCard>, labels: &ContextLabels) -> ParseOutcome {
    let mut outcome = ParseOutcome::default();

    for card in cards {
        let front = card.front.trim().to_string();
        let back = card.back.trim().to_string();
        if front.is_empty() || back.is_empty() {
            continue;
        }

        let chunk_id = labels.chunk(card.label).map(|chunk| chunk.chunk_id);
        if chunk_id.is_none() && !outcome.dropped_labels.contains(&card.label) {
            outcome.dropped_labels.push(card.label);
        }

        outcome.cards.push(ResolvedCard {
            front,
            back,
            chunk_id,
            label: card.label,
        });
    }

    outcome.dropped_labels.sort_unstable();
    outcome
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `cargo test -p stvdy-core --test generation_flashcards`
Expected: PASS (7 tests).

- [ ] **Step 5: Commit**

```bash
git add crates/core/src/generation crates/core/tests/generation_flashcards.rs
git commit -m "feat(core): the flashcards JSON contract, parsed and label-resolved"
```

---

## Task 4: Decks, the generation job, and the refund path

**Files:**
- Create: `migrations/0021_flashcard_generation.sql`
- Create: `crates/server/src/flashcards/{mod.rs,routes.rs,generate.rs}`
- Create: `crates/server/src/jobs/handlers/generate.rs`
- Modify: `crates/server/src/{lib.rs,routes/mod.rs,limits.rs,util.rs,credits.rs,chat/routes.rs,chat/prompt.rs,workspaces.rs}`
- Modify: `crates/server/src/jobs/{worker.rs,handlers/mod.rs}`
- Test: `crates/server/tests/flashcards.rs`

**Interfaces:**
- Consumes: `compose_task`, `ModuleId`, `parse_cards`, `resolve_cards`,
  `is_retryable`, `credits::spend`, `credits::record_provider_usage`,
  `limits::enforce_batch`, `jobs::enqueue`, `Failure`.
- Produces: `flashcards::routes()`, `jobs::handlers::generate::run`,
  `credits::refund`, `limits::enforce_batch`, `util::local_month_start`,
  `workspaces::owned_name`, `chat::prompt::base_vars`.
  Consumed by Task 5 (reviews) and Task 6 (the SPA).

- [ ] **Step 1: Write the migration**

```sql
-- Generation is asynchronous with visible progress, so the deck carries the state
-- the client polls. `credits_spent` exists so a generation that fails for good can
-- refund exactly what was charged, once; `usage_event_id` carries the row the
-- provider's cost and token counts are attached to, which the margin report needs
-- (A41) and which only the spending route can create.
alter table decks
  add column status text not null default 'ready',
  add column error text,
  add column requested_count int not null default 0,
  add column credits_spent int not null default 0,
  add column usage_event_id uuid;

-- A deck is created `generating`; the default is `ready` so any row written by a
-- path that does not generate (a future hand-made deck) is usable immediately.
```

- [ ] **Step 2: Write the failing test**

`crates/server/tests/flashcards.rs` (first slice: generation):

```rust
//! Phase 4 flashcards: generation, the JSON contract, review replay, due cards.
//!
//! Runs against `ScriptedLlm`, so the JSON the model returns is scripted and the
//! suite asserts on both halves of the exchange: what the prompt contained (via
//! `requests()`) and what was stored.

#![allow(clippy::unwrap_used, clippy::expect_used, clippy::panic)]

use std::sync::Arc;

use axum::http::StatusCode;
use sqlx::PgPool;
use stvdy_core::traits::testing::ScriptedLlm;
use stvdy_server::testing::TestState;

mod common;
use common::{drain_jobs, json, post_json, signed_up, workspace_with_text};

const INDONESIAN: &str = "TCP memakai three-way handshake untuk membuka koneksi. \
UDP tidak memakai handshake sehingga lebih cepat tetapi tidak menjamin urutan paket.";

/// One valid reply, as `prompts/flashcards.v1.md` documents it.
fn reply(front: &str, back: &str, label: u32) -> String {
    serde_json::json!({ "cards": [{ "front": front, "back": back, "label": label }] }).to_string()
}

async fn deck_created(
    app: &axum::Router,
    workspace_id: &str,
    token: &str,
    source_id: &str,
    count: u32,
) -> (StatusCode, serde_json::Value) {
    json(
        app,
        "POST",
        &format!("/workspaces/{workspace_id}/decks"),
        token,
        serde_json::json!({
            "title": "Jaringan",
            "source_ids": [source_id],
            "count": count,
        }),
    )
    .await
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_deck_generates_cards_linked_to_their_chunk(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply(
        "Apa fungsi three-way handshake?",
        "Membuka koneksi TCP.",
        1,
    )]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "deck@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 10).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    assert_eq!(body["deck"]["status"], "generating");

    drain_jobs(&harness).await;

    let (status, deck) = json(&app, "GET", &format!("/decks/{deck_id}"), &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{deck}");
    assert_eq!(deck["deck"]["status"], "ready", "{deck}");
    assert_eq!(deck["deck"]["card_count"], 1);
    let card = &deck["cards"][0];
    assert_eq!(card["front"], "Apa fungsi three-way handshake?");
    // The card links to a real chunk of the Indonesian source.
    assert!(!card["chunk_id"].is_null(), "{card}");

    // And the prompt that produced it carried the label block and the count.
    let requests = llm.requests();
    assert_eq!(requests.len(), 1);
    let prompt = &requests[0].messages[0].content;
    assert!(prompt.contains("[1]"), "no label block: {prompt}");
    assert!(prompt.contains("10 cards requested"), "{prompt}");
    assert!(prompt.contains("the same language as the source material"), "{prompt}");
    assert!(requests[0].json_mode, "generation must ask for JSON");
}

#[sqlx::test(migrations = "../../migrations")]
async fn an_invalid_reply_is_retried_once_and_then_succeeds(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        "Sure! Here are your cards.".to_string(),
        reply("Apa itu UDP?", "Protokol tanpa koneksi.", 1),
    ]));
    let harness = harness.with_llm(llm.clone());
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "retry@example.test").await;
    let (workspace_id, source_id) = workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 5).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(&app, "GET", &format!("/decks/{deck_id}"), &token, serde_json::Value::Null).await;
    assert_eq!(deck["deck"]["status"], "ready", "{deck}");
    assert_eq!(llm.requests().len(), 2, "the second attempt must happen");
    // The retry carries the bad reply and a correction, not the same prompt twice.
    let second = &llm.requests()[1].messages;
    // The retry carries the original system prompt, the bad reply as an assistant
    // turn, and the correction. Three messages, not a second copy of the system
    // prompt: repeating the instruction unchanged is what makes a model answer the
    // same way again.
    assert_eq!(second.len(), 3, "{second:?}");
    assert_eq!(second[1].content, "Sure! Here are your cards.");
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_generation_that_fails_for_good_refunds_the_credits(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        "not json".to_string(),
        "still not json".to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "refund@example.test").await;
    let (workspace_id, source_id) = workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let before: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'refund@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();

    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 10).await;
    assert_eq!(status, StatusCode::CREATED, "{body}");
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(&app, "GET", &format!("/decks/{deck_id}"), &token, serde_json::Value::Null).await;
    assert_eq!(deck["deck"]["status"], "failed", "{deck}");
    assert!(
        deck["deck"]["error"].as_str().unwrap().contains("JSON"),
        "the error must say what happened: {deck}"
    );

    let after: i64 = sqlx::query_scalar(
        "select coalesce(sum(delta), 0)::bigint from credit_ledger
          where user_id = (select id from users where email = 'refund@example.test')",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(after, before, "the 4 credits must come back exactly once");
}

/// A failed generation leaves an honest `usage_events` row: the action happened, the
/// provider was called, and the prompt version is recorded. A row that vanishes when
/// the job fails would hide a real cost from the margin report (A41).
#[sqlx::test(migrations = "../../migrations")]
async fn a_failed_generation_still_records_its_usage_row(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![
        "not json".to_string(),
        "still not json".to_string(),
    ]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "usage@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 10).await;
    let _deck_id = body["deck"]["id"].as_str().unwrap();
    drain_jobs(&harness).await;

    let (action, prompt_version): (String, Option<String>) = sqlx::query_as(
        "select action, prompt_version from usage_events
          where user_id = (select id from users where email = 'usage@example.test')
            and action = 'generate_flashcards'
          order by created_at desc limit 1",
    )
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(action, "generate_flashcards");
    assert_eq!(
        prompt_version.as_deref(),
        Some("flashcards.v1"),
        "a model call must be attributable to a prompt version"
    );
}

#[sqlx::test(migrations = "../../migrations")]
async fn a_card_whose_label_was_not_sent_is_stored_without_a_chunk(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Apa itu TCP?", "Protokol.", 99)]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "label@example.test").await;
    let (workspace_id, source_id) = workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 5).await;
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(&app, "GET", &format!("/decks/{deck_id}"), &token, serde_json::Value::Null).await;
    // The deck is still usable: a fabricated label costs the link, not the card.
    assert_eq!(deck["deck"]["status"], "ready", "{deck}");
    assert!(deck["cards"][0]["chunk_id"].is_null(), "{deck}");
}

#[sqlx::test(migrations = "../../migrations")]
async fn the_monthly_card_limit_is_enforced(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "limit@example.test").await;
    let (workspace_id, source_id) = workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;

    // Free allows 30 a month; 31 in one request is refused before anything is spent.
    let (status, body) = deck_created(&app, &workspace_id, &token, &source_id, 31).await;
    assert_eq!(status, StatusCode::UNPROCESSABLE_ENTITY, "{body}");
    assert_eq!(body["code"], "limit_reached", "{body}");
    assert_eq!(body["limit"], 30);
}
```

Add `workspace_with_text` to `crates/server/tests/common/mod.rs` if it is not already
exported (the chat suite uses `workspace_with_source`, a local helper; `common` has
`workspace_with_text` at line 272 — confirm its signature and reuse it, adjusting
the call if it returns only an id).

- [ ] **Step 3: Run the tests to verify they fail**

Run: `cargo test -p stvdy-server --test flashcards`
Expected: FAIL, `POST /workspaces/:id/decks` is a 404.

- [ ] **Step 4: Implement the shared helpers**

`crates/server/src/util.rs` — add:

```rust
/// Midnight on the first of the user's current local month, as UTC.
///
/// The monthly counters (`flashcards_per_month`, and Phase 7's others) reset on a
/// local calendar boundary, never a rolling 30-day window: "30 cards a month" means
/// the month the student is in, and a rolling window would let a student who waits
/// two days get 60. `chrono-tz`, never a fixed offset (A6).
pub fn local_month_start(timezone: &str, clock: &dyn Clock) -> DateTime<Utc> {
    let tz: Tz = timezone.parse().unwrap_or(chrono_tz::UTC);
    let now_local = clock.now().with_timezone(&tz);

    use chrono::TimeZone;
    let first = now_local
        .date_naive()
        .with_day(1)
        .unwrap_or(now_local.date_naive())
        .and_hms_opt(0, 0, 0)
        .and_then(|naive| tz.from_local_datetime(&naive).earliest())
        .unwrap_or(now_local);

    first.with_timezone(&Utc)
}
```

with tests mirroring `local_day_start`'s (Jakarta on the 11th → the 1st at 17:00Z of
the previous day; Berlin across the March DST change).

`crates/server/src/limits.rs` — add a batch-aware variant, because a deck request
asks for many cards at once and `enforce_count` only expresses "one more":

```rust
/// A countable limit where more than one is requested at a time.
///
/// `enforce_count` asks "is there room for one more", which is the right question
/// for a workspace or a device. A deck asks for `count` cards in one request, and
/// checking them one at a time would let 30 requests of 30 cards each through.
pub fn enforce_batch(
    plans: &PlansFile,
    plan: PlanId,
    limit: Limit,
    used: i64,
    requested: i64,
    feature: &'static str,
) -> Result<(), AppError> {
    let _ = plans;
    match limit.max() {
        None => Ok(()),
        Some(max) => {
            if used + requested <= i64::from(max) {
                Ok(())
            } else {
                Err(limit_reached(plan, limit, used, feature))
            }
        }
    }
}
```

`crates/server/src/workspaces.rs` — move the ownership helper out of the chat module
so flashcards (and Phase 5+) use one implementation:

```rust
/// The workspace's name, or 404 when it is not this user's.
///
/// Every workspace-scoped route needs this, and a second copy is how one of them
/// eventually forgets the `user_id` predicate, which is a cross-tenant read.
pub async fn owned_name(state: &AppState, user_id: Uuid, workspace_id: Uuid) -> AppResult<String> {
    // body moved verbatim from `chat::routes::owned_workspace_name`
}
```

Update `crates/server/src/chat/routes.rs` to call `crate::workspaces::owned_name` and
delete the private copy.

`crates/server/src/credits.rs` — add the refund:

```rust
/// Reverses a spend that could not be delivered.
///
/// A generation that fails for good has already charged the user and bought
/// nothing. The row is a positive `credit_ledger` entry with the documented
/// `refund` reason and `ref_id` naming what failed, so the ledger explains itself
/// to the admin margin report instead of showing an unexplained balance change.
/// `monthly` is the bucket because that is the one `spend` consumes first.
pub async fn refund(
    pool: &PgPool,
    user_id: Uuid,
    credits: i64,
    ref_id: Uuid,
    reason: &str,
) -> AppResult<()> {
    if credits <= 0 {
        return Ok(());
    }
    sqlx::query(
        "insert into credit_ledger (user_id, delta, bucket, reason, ref_id)
         values ($1, $2, 'monthly', $3, $4)",
    )
    .bind(user_id)
    .bind(credits)
    .bind(reason)
    .bind(ref_id)
    .execute(pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("refunding credits: {error}")))?;
    Ok(())
}
```

`crates/server/src/chat/prompt.rs` — extract the base variables so the generation
job builds them the same way the chat route does:

```rust
/// The base-prompt variables for one request.
///
/// Extracted because a task prompt is composed on the same trimmed base, and two
/// builders would eventually disagree about `{{platform}}` or `{{plan_name}}`. The
/// disagreement would show up as a prompt describing the wrong app, which is not
/// something a test would catch.
///
/// It takes the individual values rather than an `AuthUser` because the generation
/// job has no `AuthUser`: it has a row. Fabricating one (with a device id and a
/// session id it does not have) would be a lie in the type system.
pub fn base_vars(
    state: &AppState,
    email: &str,
    timezone: &str,
    plan: stvdy_core::config::PlanId,
    study_mode: bool,
    use_web: bool,
    workspace_name: &str,
) -> ChatVars {
    let plan_config = state.config.plans.get(plan);
    let now_local = state
        .clock
        .now()
        .with_timezone(&timezone.parse().unwrap_or(chrono_tz::UTC));

    ChatVars {
        assistant_name: ASSISTANT_NAME.to_string(),
        app_name: APP_NAME.to_string(),
        now: now_local.format("%Y-%m-%d %H:%M %Z").to_string(),
        timezone: timezone.to_string(),
        // The prompt tells the model to keep phone replies short, so this has to be
        // the real platform rather than a guess.
        platform: "unknown".to_string(),
        plan_name: plan_config.name.clone(),
        study_mode: if study_mode { "on" } else { "off" }.to_string(),
        study_hours_summary: "not set".to_string(),
        use_web: if use_web { "on" } else { "off" }.to_string(),
        allow_upgrade_hint: state
            .config
            .billing_policy
            .purchase_allowed(stvdy_core::config::Platform::Web),
        user_name: email.to_string(),
        workspace_name: workspace_name.to_string(),
    }
}
```

and `compose` becomes:

```rust
    let vars = base_vars(
        state,
        &user.email,
        &user.timezone,
        user.plan(),
        study_mode,
        use_web,
        workspace_name,
    );
```

replacing the `let now_local = ...` and `let vars = ChatVars { ... };` blocks
verbatim. `plan_config` is no longer needed in `compose`; delete it.

**Also in this step:** `crates/core/src/config/plans.rs` gains the parse that today
lives inline in `AuthUser::plan()`:

```rust
    /// The plan a stored `users.plan_id` names. An unrecognised value is `Free`,
    /// which is the safe direction: a corrupt or future plan string must not grant
    /// more than the free tier.
    pub fn parse(raw: &str) -> PlanId {
        match raw {
            "pro" => PlanId::Pro,
            "power" => PlanId::Power,
            _ => PlanId::Free,
        }
    }
```

and `AuthUser::plan()` becomes `stvdy_core::config::PlanId::parse(&self.plan_id)`.
One implementation, because a second copy is how a plan string starts meaning two
different things.

- [ ] **Step 5: Implement the flashcards module**

`crates/server/src/flashcards/mod.rs`:

```rust
//! Decks, cards and reviews.
//!
//! Split three ways: `routes` owns the HTTP surface, `generate` owns the prompt and
//! the insert for one generation, and the review path lives in `routes` because it
//! is short and is the only place that replays SM-2 in a transaction.

pub mod generate;
pub mod routes;

pub use routes::routes;
```

`crates/server/src/flashcards/routes.rs`:

```rust
//! `POST|GET /workspaces/:id/decks`, `GET /decks/:id`, `GET /decks/:id/due`,
//! `POST /flashcards/:id/review`.
//!
//! The order of operations in `create_deck` is the contract: verified email, owned
//! workspace, owned and ready sources, monthly card limit, credit spend, insert the
//! deck, enqueue the job. Every check that can fail runs before the spend, so a
//! refused request costs nothing.

use axum::Json;
use axum::Router;
use axum::extract::{Path, Query, State};
use axum::http::StatusCode;
use axum::routing::{get, post};
use chrono::{DateTime, Utc};
use serde::{Deserialize, Serialize};
// `Row` is required for `row.get::<T, _>("column")`; a trait object's methods resolve
// without an import, but a `PgRow`'s do not.
use sqlx::Row;
use uuid::Uuid;

use stvdy_core::config::Limit;
use stvdy_core::sm2::{CardState, Rating, Review, replay};

use crate::auth::extract::{AuthUser, VerifiedUser};
use crate::error::{AppError, AppResult};
use crate::state::AppState;
use crate::{credits, jobs, limits, util, workspaces};

/// The action name in `usage_events` and the ledger, so a cost report can separate
/// generation from chat.
const GENERATE_ACTION: &str = "generate_flashcards";

/// The most cards one request may ask for.
///
/// The credit cost is quoted from the requested count, so accepting more than the
/// generator will produce would overcharge. A larger request is refused rather
/// than truncated.
pub const MAX_REQUESTED_CARDS: u32 = 50;

/// The most sources one deck may draw on. A deck from twenty sources is a topic,
/// not a deck, and the prompt has a chunk budget.
const MAX_SOURCES: usize = 20;

pub fn routes() -> Router<AppState> {
    Router::new()
        .route("/workspaces/{id}/decks", post(create_deck).get(list_decks))
        .route("/decks/{id}", get(get_deck))
        .route("/decks/{id}/due", get(due_cards))
        .route("/flashcards/{id}/review", post(review_card))
}

#[derive(Debug, Deserialize)]
pub struct CreateDeckBody {
    pub title: String,
    pub source_ids: Vec<Uuid>,
    /// How many cards to ask for. The generator may return fewer; it is told never
    /// to pad.
    pub count: u32,
}

#[derive(Debug, Serialize)]
struct DeckJson {
    id: Uuid,
    workspace_id: Uuid,
    title: String,
    source_ids: Vec<Uuid>,
    status: String,
    error: Option<String>,
    requested_count: i32,
    card_count: i64,
    created_at: DateTime<Utc>,
}

#[derive(Debug, Serialize)]
struct CardJson {
    id: Uuid,
    deck_id: Uuid,
    front: String,
    back: String,
    chunk_id: Option<Uuid>,
    ease: f32,
    interval_days: i32,
    due_at: DateTime<Utc>,
}

/// `POST /workspaces/:id/decks`. 201 with the deck in status `generating`.
pub async fn create_deck(
    State(state): State<AppState>,
    user: VerifiedUser,
    Path(workspace_id): Path<Uuid>,
    Json(body): Json<CreateDeckBody>,
) -> AppResult<(StatusCode, Json<serde_json::Value>)> {
    let title = body.title.trim().to_string();
    if title.is_empty() || title.chars().count() > 120 {
        return Err(AppError::Validation(
            "the deck title must be between 1 and 120 characters".to_string(),
        ));
    }
    if body.count == 0 || body.count > MAX_REQUESTED_CARDS {
        return Err(AppError::Validation(format!(
            "count must be between 1 and {MAX_REQUESTED_CARDS}"
        )));
    }
    if body.source_ids.is_empty() || body.source_ids.len() > MAX_SOURCES {
        return Err(AppError::Validation(format!(
            "a deck is generated from between 1 and {MAX_SOURCES} sources"
        )));
    }

    workspaces::owned_name(&state, user.user_id, workspace_id).await?;

    // Every source must be this user's, in this workspace, and finished ingesting:
    // generating from a source whose chunks do not exist yet would produce a deck
    // with no cards and no explanation.
    let ready: i64 = sqlx::query_scalar(
        "select count(*)::bigint from sources
          where id = any($1) and workspace_id = $2 and user_id = $3
            and status = 'ready' and deleted_at is null",
    )
    .bind(&body.source_ids)
    .bind(workspace_id)
    .bind(user.user_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("checking the deck's sources: {error}")))?;
    if ready != body.source_ids.len() as i64 {
        return Err(AppError::Validation(
            "every source must be one of this workspace's, and finished indexing".to_string(),
        ));
    }

    let plan = user.plan();
    let month_start = util::local_month_start(&user.timezone, state.clock.as_ref());
    let used: i64 = sqlx::query_scalar(
        "select count(*)::bigint from flashcards
          where user_id = $1 and deleted_at is null and created_at >= $2",
    )
    .bind(user.user_id)
    .bind(month_start)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("counting this month's cards: {error}")))?;

    limits::enforce_batch(
        &state.config.plans,
        plan,
        state.config.plans.get(plan).flashcards_per_month,
        used,
        i64::from(body.count),
        "flashcards",
    )?;

    // The cost is per ten cards, from the config: `flashcard_batch_10 = 4` is a
    // batch price, and asking for 21 cards is three batches.
    let batches = i64::from(body.count).div_ceil(10);
    let cost = batches * state.config.plans.credits.flashcard_batch_10;
    let spent = credits::spend(&state, user.user_id, plan, GENERATE_ACTION, cost, None).await?;

    let deck_id: Uuid = sqlx::query_scalar(
        "insert into decks
            (user_id, workspace_id, title, source_ids, status, requested_count, credits_spent, usage_event_id)
         values ($1, $2, $3, $4, 'generating', $5, $6, $7)
         returning id",
    )
    .bind(user.user_id)
    .bind(workspace_id)
    .bind(&title)
    .bind(&body.source_ids)
    .bind(body.count as i32)
    .bind(cost as i32)
    .bind(spent.usage_event_id)
    .fetch_one(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("creating the deck: {error}")))?;

    // The job is enqueued after the spend and after the row exists, so a failure
    // here leaves a deck the user can see failing rather than a charge with no
    // trace. The refund happens because a deck that will never generate must not
    // keep the credits.
    if let Err(error) =
        jobs::enqueue(&state.pool, &state.jobs, "generate_flashcards", serde_json::json!({ "deck_id": deck_id }), None)
            .await
    {
        generate::fail_and_refund(
            &state,
            deck_id,
            user.user_id,
            cost,
            "the generation job could not be queued",
        )
        .await;
        return Err(AppError::Internal(anyhow::anyhow!(
            "enqueueing the generation job: {error}"
        )));
    }

    Ok((
        StatusCode::CREATED,
        Json(serde_json::json!({
            "deck": {
                "id": deck_id,
                "workspace_id": workspace_id,
                "title": title,
                "source_ids": body.source_ids,
                "status": "generating",
                "error": serde_json::Value::Null,
                "requested_count": body.count,
                "card_count": 0,
            }
        })),
    ))
}
```

`list_decks`, `get_deck`, `due_cards` and `review_card` are the rest of this file:

```rust
/// `GET /workspaces/:id/decks`, newest first, tombstones excluded.
///
/// Added beyond the spec's route list: without it a deck is unreachable after a
/// page reload, which makes the phase's own exit gate untestable by hand (A75).
pub async fn list_decks(
    State(state): State<AppState>,
    user: AuthUser,
    Path(workspace_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    workspaces::owned_name(&state, user.user_id, workspace_id).await?;
    let rows = sqlx::query(
        "select d.id, d.workspace_id, d.title, d.source_ids, d.status, d.error,
                d.requested_count, d.created_at,
                (select count(*) from flashcards f
                  where f.deck_id = d.id and f.deleted_at is null) as card_count
           from decks d
          where d.user_id = $1 and d.workspace_id = $2 and d.deleted_at is null
          order by d.created_at desc, d.id",
    )
    .bind(user.user_id)
    .bind(workspace_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("listing decks: {error}")))?;

    let decks: Vec<DeckJson> = rows.iter().map(deck_from_row).collect();
    Ok(Json(serde_json::json!({ "decks": decks })))
}

/// `GET /decks/:id`: the deck, its cards, and its generation state.
pub async fn get_deck(
    State(state): State<AppState>,
    user: AuthUser,
    Path(deck_id): Path<Uuid>,
) -> AppResult<Json<serde_json::Value>> {
    let deck = owned_deck(&state, user.user_id, deck_id).await?;
    let cards = sqlx::query(
        "select id, deck_id, front, back, chunk_id, ease, interval_days, due_at
           from flashcards
          where deck_id = $1 and deleted_at is null
          order by created_at, id",
    )
    .bind(deck_id)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the deck's cards: {error}")))?;

    let cards: Vec<CardJson> = cards.iter().map(card_from_row).collect();
    let mut deck = deck;
    deck.card_count = cards.len() as i64;
    Ok(Json(serde_json::json!({ "deck": deck, "cards": cards })))
}

#[derive(Debug, Deserialize)]
pub struct DueQuery {
    #[serde(default)]
    pub limit: Option<i64>,
}

/// `GET /decks/:id/due`: the study queue, soonest first.
///
/// Ordered by `(due_at, id)` so a refetch cannot reshuffle the queue under the
/// student's finger, and capped so a deck that has never been studied does not
/// return a thousand cards in one response.
pub async fn due_cards(
    State(state): State<AppState>,
    user: AuthUser,
    Path(deck_id): Path<Uuid>,
    Query(query): Query<DueQuery>,
) -> AppResult<Json<serde_json::Value>> {
    owned_deck(&state, user.user_id, deck_id).await?;
    let limit = query.limit.unwrap_or(20).clamp(1, 100);
    let now = state.clock.now();

    let cards = sqlx::query(
        "select f.id, f.deck_id, f.front, f.back, f.chunk_id, f.ease, f.interval_days, f.due_at
           from flashcards f
          where f.deck_id = $1 and f.deleted_at is null and f.due_at <= $2
          order by f.due_at asc, f.id asc
          limit $3",
    )
    .bind(deck_id)
    .bind(now)
    .bind(limit)
    .fetch_all(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading due cards: {error}")))?;

    let cards: Vec<CardJson> = cards.iter().map(card_from_row).collect();
    Ok(Json(serde_json::json!({ "cards": cards, "now": now })))
}

#[derive(Debug, Deserialize)]
pub struct ReviewBody {
    /// Client-generated, so a retry after a lost response cannot apply twice.
    #[serde(default)]
    pub id: Option<Uuid>,
    pub rating: String,
    /// The device's clock. Absent means the server's, which is what an online
    /// review uses.
    #[serde(default)]
    pub reviewed_at: Option<DateTime<Utc>>,
}

/// A review further in the future than this is refused: a device with a wrong
/// clock would otherwise pin a card out of the queue for months.
const FUTURE_REVIEW_TOLERANCE_SECS: i64 = 24 * 60 * 60;

/// `POST /flashcards/:id/review`.
///
/// The whole point of the phase: the review is inserted, the card's state is
/// replayed from *every* review in `(reviewed_at, id)` order, and the result is
/// written back. Two devices therefore converge on one state rather than on
/// whichever wrote last.
pub async fn review_card(
    State(state): State<AppState>,
    user: AuthUser,
    Path(card_id): Path<Uuid>,
    Json(body): Json<ReviewBody>,
) -> AppResult<Json<serde_json::Value>> {
    let rating = Rating::parse(&body.rating).map_err(|error| AppError::Validation(error.to_string()))?;
    let now = state.clock.now();
    let reviewed_at = body.reviewed_at.unwrap_or(now);
    if reviewed_at > now + chrono::Duration::seconds(FUTURE_REVIEW_TOLERANCE_SECS) {
        return Err(AppError::Validation(
            "the review's timestamp is in the future; check the device clock".to_string(),
        ));
    }
    let review_id = body.id.unwrap_or_else(Uuid::new_v4);

    let mut tx = state
        .pool
        .begin()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("beginning a review: {error}")))?;

    // The same advisory lock the spend and the sync triggers take, and the same
    // reason: two devices reviewing one card concurrently would each replay only
    // their own review, and the second `update` would silently drop the other.
    sqlx::query("select pg_advisory_xact_lock(hashtext($1::text))")
        .bind(user.user_id.to_string())
        .execute(&mut *tx)
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("taking the review lock: {error}")))?;

    let card = sqlx::query(
        "select f.id, f.created_at
           from flashcards f
           join decks d on d.id = f.deck_id
          where f.id = $1 and d.user_id = $2 and f.deleted_at is null and d.deleted_at is null",
    )
    .bind(card_id)
    .bind(user.user_id)
    .fetch_optional(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the card: {error}")))?
    .ok_or(AppError::NotFound)?;

    let created_at: DateTime<Utc> = card.get("created_at");

    let inserted = sqlx::query(
        "insert into flashcard_reviews (id, user_id, flashcard_id, rating, reviewed_at, device_id)
         values ($1, $2, $3, $4, $5, $6)
         on conflict (id) do nothing",
    )
    .bind(review_id)
    .bind(user.user_id)
    .bind(card_id)
    .bind(rating.as_str())
    .bind(reviewed_at)
    .bind(user.device_id)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("writing the review: {error}")))?
    .rows_affected()
        > 0;

    // Replayed from the full history, inside the lock, so the stored state is a
    // function of the rows and not of this request's arrival order.
    let rows = sqlx::query(
        "select id, rating, reviewed_at from flashcard_reviews
          where flashcard_id = $1 order by reviewed_at asc, id asc",
    )
    .bind(card_id)
    .fetch_all(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the reviews: {error}")))?;

    let mut reviews = Vec::with_capacity(rows.len());
    for row in &rows {
        let raw: String = row.get("rating");
        // A stored rating the enum cannot read is a data defect, not a user error.
        // Skipping it would silently change the schedule, so this fails loudly.
        let rating = Rating::parse(&raw)
            .map_err(|_| AppError::Internal(anyhow::anyhow!("stored review rating {raw:?} is not one of again/hard/good/easy")))?;
        reviews.push(Review {
            id: row.get("id"),
            rating,
            reviewed_at: row.get("reviewed_at"),
        });
    }

    let CardState { ease, interval_days, due_at } = replay(created_at, &reviews);

    sqlx::query(
        "update flashcards
            set ease = $2, interval_days = $3, due_at = $4, updated_at = now()
          where id = $1",
    )
    .bind(card_id)
    .bind(ease)
    .bind(interval_days)
    .bind(due_at)
    .execute(&mut *tx)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("updating the card: {error}")))?;

    tx.commit()
        .await
        .map_err(|error| AppError::Internal(anyhow::anyhow!("committing a review: {error}")))?;

    Ok(Json(serde_json::json!({
        "applied": inserted,
        "card": {
            "id": card_id,
            "ease": ease,
            "interval_days": interval_days,
            "due_at": due_at,
        }
    })))
}
```

Plus the helpers in the same file:

```rust
/// The deck, or 404 when it is not this user's. Tombstones are excluded, so a
/// deleted deck is indistinguishable from one that never existed.
async fn owned_deck(state: &AppState, user_id: Uuid, deck_id: Uuid) -> AppResult<DeckJson> {
    let row = sqlx::query(
        "select d.id, d.workspace_id, d.title, d.source_ids, d.status, d.error,
                d.requested_count, d.created_at,
                (select count(*) from flashcards f
                  where f.deck_id = d.id and f.deleted_at is null) as card_count
           from decks d
          where d.id = $1 and d.user_id = $2 and d.deleted_at is null",
    )
    .bind(deck_id)
    .bind(user_id)
    .fetch_optional(&state.pool)
    .await
    .map_err(|error| AppError::Internal(anyhow::anyhow!("loading the deck: {error}")))?;

    row.as_ref().map(deck_from_row).ok_or(AppError::NotFound)
}

fn deck_from_row(row: &sqlx::postgres::PgRow) -> DeckJson {
    DeckJson {
        id: row.get("id"),
        workspace_id: row.get("workspace_id"),
        title: row.get("title"),
        source_ids: row.get("source_ids"),
        status: row.get("status"),
        error: row.get("error"),
        requested_count: row.get("requested_count"),
        card_count: row.get("card_count"),
        created_at: row.get("created_at"),
    }
}

fn card_from_row(row: &sqlx::postgres::PgRow) -> CardJson {
    CardJson {
        id: row.get("id"),
        deck_id: row.get("deck_id"),
        front: row.get("front"),
        back: row.get("back"),
        chunk_id: row.get("chunk_id"),
        ease: row.get("ease"),
        interval_days: row.get("interval_days"),
        due_at: row.get("due_at"),
    }
}
```

- [ ] **Step 6: Implement the generation job**

`crates/server/src/flashcards/generate.rs`:

```rust
//! `generate_flashcards`: the prompt, the one retry, the insert, and the refund.
//!
//! The retry is here rather than in the provider because it is a *contract*
//! decision: a reply that is not JSON is worth one corrective attempt, and a reply
//! that is valid JSON with no cards is not.

use sqlx::Row;
use stvdy_core::prompts::{ChatVars, ModuleId, PromptChunk, compose_task};
use stvdy_core::generation::flashcards::{parse_cards, resolve_cards};
use stvdy_core::generation::is_retryable;
use stvdy_core::traits::llm::{ChatMsg, ChatRequest, Role};
use uuid::Uuid;

use crate::credits;
use crate::error::AppResult;
use crate::jobs::Failure;
use crate::state::AppState;

/// How many chunks of the selected sources the prompt carries.
///
/// Deterministic (`order by source_id, idx`) and bounded, because the prompt has a
/// token budget and the same deck must produce the same prompt twice.
pub const GENERATION_CHUNK_LIMIT: i64 = 40;

/// Low, because a card must be faithful to the material; not zero, because a fully
/// deterministic model produces near-duplicate cards.
const TEMPERATURE: f32 = 0.2;
const MAX_TOKENS: u32 = 2000;

/// What the second attempt says. Short and specific: the model has the bad reply
/// in its own context, so repeating the whole instruction is what makes a model
/// answer the same way again.
const CORRECTION: &str =
    "Your previous reply was not a valid JSON object matching the schema. Reply with only the \
     JSON object, with no prose before or after it and no markdown fences.";

pub struct Generation {
    pub cards: Vec<ResolvedCardInsert>,
    pub dropped_labels: Vec<u32>,
    pub in_tokens: u32,
    pub out_tokens: u32,
    pub provider_cost_usd: Option<f64>,
    pub prompt_version: String,
}

pub struct ResolvedCardInsert {
    pub front: String,
    pub back: String,
    pub chunk_id: Option<Uuid>,
}

/// Runs one generation for one deck. Returns the cards to insert.
pub async fn generate(state: &AppState, deck: &DeckRow) -> Result<Generation, Failure> {
    let provider = state.llm.clone().ok_or_else(|| {
        Failure::permanent("no model provider is configured on this server (LLM_BASE_URL is unset)")
    })?;

    let chunks = load_chunks(state, deck).await?;
    if chunks.is_empty() {
        return Err(Failure::permanent(
            "the selected sources have no indexed text to generate from",
        ));
    }

    let user = load_user(state, deck.user_id).await?;
    let base: ChatVars = crate::chat::prompt::base_vars(state, &user, &deck.workspace_name);
    let mut module_vars = std::collections::BTreeMap::new();
    module_vars.insert("count", deck.requested_count.to_string());
    // `output_language` is deliberately absent: the module declares it optional and
    // defaults to the material's language, which is the right behaviour until there
    // is a user language setting (Phase 9).
    let composed = compose_task(ModuleId::FlashcardsV1, &base, &module_vars, &chunks)
        .map_err(|error| Failure::permanent(format!("composing the prompt: {error}")))?;

    let request = ChatRequest {
        model: String::new(),
        messages: vec![ChatMsg {
            role: Role::System,
            content: composed.text.clone(),
        }],
        tools: Vec::new(),
        temperature: TEMPERATURE,
        max_tokens: MAX_TOKENS,
        json_mode: true,
    };

    let first = provider
        .complete(request.clone())
        .await
        .map_err(|error| Failure::transient(format!("calling the model: {error}")))?;

    let (cards, in_tokens, out_tokens, provider_cost_usd) = match parse_cards(&first.text) {
        Ok(cards) => (cards, first.usage.in_tokens, first.usage.out_tokens, first.usage.provider_cost_usd),
        Err(error) if is_retryable(&error) => {
            let mut retry = request.clone();
            retry.messages.push(ChatMsg {
                role: Role::Assistant,
                content: first.text.clone(),
            });
            retry.messages.push(ChatMsg {
                role: Role::User,
                content: CORRECTION.to_string(),
            });
            let second = provider
                .complete(retry)
                .await
                .map_err(|error| Failure::transient(format!("retrying the model call: {error}")))?;
            match parse_cards(&second.text) {
                Ok(cards) => (
                    cards,
                    first.usage.in_tokens + second.usage.in_tokens,
                    first.usage.out_tokens + second.usage.out_tokens,
                    sum_costs(first.usage.provider_cost_usd, second.usage.provider_cost_usd),
                ),
                Err(error) => {
                    return Err(Failure::permanent(format!(
                        "the model did not return usable JSON after a retry: {error}"
                    )))
                }
            }
        }
        Err(error) => return Err(Failure::permanent(format!("the model's reply was unusable: {error}"))),
    };

    let outcome = resolve_cards(cards, &composed.labels);

    Ok(Generation {
        cards: outcome
            .cards
            .into_iter()
            .map(|card| ResolvedCardInsert {
                front: card.front,
                back: card.back,
                chunk_id: card.chunk_id,
            })
            .collect(),
        dropped_labels: outcome.dropped_labels,
        in_tokens,
        out_tokens,
        provider_cost_usd,
        prompt_version: composed.prompt_version,
    })
}

/// Sums two optional costs, keeping `None` for "unknown" rather than treating it
/// as zero: recording zero understates spend in the margin report (A41).
fn sum_costs(first: Option<f64>, second: Option<f64>) -> Option<f64> {
    match (first, second) {
        (None, None) => None,
        (a, b) => Some(a.unwrap_or(0.0) + b.unwrap_or(0.0)),
    }
}
```

`crates/server/src/jobs/handlers/generate.rs`:

```rust
//! `generate_flashcards`: the job shell around `flashcards::generate`.
//!
//! It owns the transaction boundary (cards plus the deck's status change), the
//! permanent-failure path (status, readable error, refund), and the give-up case:
//! a transient failure on the last allowed attempt is final, and the worker's
//! `mark_failed` is the only place that knows it, so the check is here.

use sqlx::Row;
use uuid::Uuid;

use crate::flashcards::generate::{self, Generation};
use crate::jobs::{self, ClaimedJob, Failure};
use crate::state::AppState;

pub async fn run(state: &AppState, job: &ClaimedJob) -> Result<(), Failure> {
    let deck_id: Uuid = job
        .payload
        .get("deck_id")
        .and_then(|value| value.as_str())
        .and_then(|raw| Uuid::parse_str(raw).ok())
        .ok_or_else(|| Failure::permanent("payload has no valid deck_id"))?;

    let deck = match load_deck(state, deck_id).await? {
        Some(deck) => deck,
        // Deleted between enqueue and claim: nothing to do, and not an error.
        None => return Ok(()),
    };

    // Idempotent: a retried job whose first attempt already wrote the cards must
    // not write a second deck's worth, and a deck the enqueue path already failed
    // must not be resurrected into `ready`. The status is the marker.
    if deck.status != "generating" {
        return Ok(());
    }

    match generate::generate(state, &deck).await {
        Ok(generation) => {
            insert_cards(state, &deck, generation).await?;
            Ok(())
        }
        Err(failure) => {
            // A transient failure on the last attempt is final in every way that
            // matters to the user: no retry is coming, so the deck must stop
            // saying "generating" and the credits must come back.
            let final_attempt = !failure.retryable || job.attempts >= jobs::MAX_ATTEMPTS;
            if final_attempt {
                fail_and_refund(state, deck.id, deck.user_id, deck.credits_spent, &failure.message)
                    .await;
            }
            Err(failure)
        }
    }
}

/// Marks the deck failed and gives back exactly what was charged, once.
///
/// The guard is the `credits_spent` zeroing in the same statement: a second call
/// (a retried job, or the enqueue-failure path in the route) refunds nothing
/// because there is nothing left to refund.
pub async fn fail_and_refund(state: &AppState, deck_id: Uuid, user_id: Uuid, credits: i32, message: &str) {
    let mut tx = match state.pool.begin().await {
        Ok(tx) => tx,
        Err(error) => {
            tracing::error!(%error, %deck_id, "could not begin the deck failure transaction");
            return;
        }
    };

    let claimed: Option<i32> = sqlx::query_scalar(
        "update decks set status = 'failed', error = $2, credits_spent = 0, updated_at = now()
          where id = $1 and credits_spent > 0
         returning credits_spent",
    )
    .bind(deck_id)
    .bind(message)
    .fetch_optional(&mut *tx)
    .await
    .ok()
    .flatten();

    if claimed.is_none() {
        // Either the deck was already failed, or it never had a charge. Make sure
        // the status is right either way, then stop.
        if let Err(error) = sqlx::query(
            "update decks set status = 'failed', error = $2, updated_at = now()
              where id = $1 and status = 'generating'",
        )
        .bind(deck_id)
        .bind(message)
        .execute(&mut *tx)
        .await
        {
            tracing::error!(%error, %deck_id, "could not mark the deck failed");
        }
    }

    if let Err(error) = tx.commit().await {
        tracing::error!(%error, %deck_id, "could not commit the deck failure");
        return;
    }

    if claimed.is_some() {
        if let Err(error) =
            crate::credits::refund(&state.pool, user_id, i64::from(credits), deck_id, "refund").await
        {
            tracing::error!(%error, %deck_id, "could not refund a failed generation");
        }
        tracing::warn!(%deck_id, credits, "generation failed permanently; credits refunded");
    }
}
```

`insert_cards` writes the cards, attaches the provider usage to the usage row, and
flips the status, in one transaction:

```rust
async fn insert_cards(state: &AppState, deck: &DeckRow, generation: Generation) -> Result<(), Failure> {
    let mut tx = state.pool.begin().await.map_err(|error| Failure::transient(format!("beginning the insert: {error}")))?;

    for card in &generation.cards {
        sqlx::query(
            "insert into flashcards (user_id, deck_id, front, back, chunk_id, due_at)
             values ($1, $2, $3, $4, $5, now())",
        )
        .bind(deck.user_id)
        .bind(deck.id)
        .bind(&card.front)
        .bind(&card.back)
        .bind(card.chunk_id)
        .execute(&mut *tx)
        .await
        .map_err(|error| Failure::transient(format!("inserting a card: {error}")))?;
    }

    sqlx::query("update decks set status = 'ready', error = null, updated_at = now() where id = $1")
        .bind(deck.id)
        .execute(&mut *tx)
        .await
        .map_err(|error| Failure::transient(format!("marking the deck ready: {error}")))?;

    tx.commit().await.map_err(|error| Failure::transient(format!("committing the deck: {error}")))?;

    if let Some(usage_event_id) = deck.usage_event_id {
        // Attached after the cards exist, because the token counts and the cost only
        // exist once the provider has answered, and the row was created before that
        // so the credit is spent whether or not the model replies.
        if let Err(error) = crate::credits::record_usage(
            &state.pool,
            Some(usage_event_id),
            Some(generation.in_tokens),
            Some(generation.out_tokens),
            generation.provider_cost_usd,
            &generation.prompt_version,
        )
        .await
        {
            // A usage row that loses its cost is a reporting gap, not a user-visible
            // failure: the cards are stored and paid for, so the job is not failed.
            tracing::error!(%error, %usage_event_id, "could not attach provider usage");
        }
    }

    if !generation.dropped_labels.is_empty() {
        tracing::warn!(
            deck_id = %deck.id,
            dropped = ?generation.dropped_labels,
            "the model cited labels that were not in the context"
        );
    }

    Ok(())
}
```

`crates/server/src/credits.rs` — rename `record_provider_usage` to `record_usage` and
generalise it. Its current shape forces the caller to supply the provider's token
counts, which a job that failed before calling the model cannot know; and a
`usage_events` row created by `spend` must be findable again by
`(action, created_at)` when there is no id to hand:

```rust
/// The `usage_events` row a spend just created, for a caller that has to look it up
/// later (a job, which has no access to the route's `Spent`).
///
/// `(user_id, action, created_at)` is unique in practice: `spend` writes exactly one
/// row per action per transaction, and the `created_at` is the spend's. The caller
/// that already has a `usage_event_id` passes it instead.
pub async fn latest_usage_event(
    pool: &PgPool,
    user_id: Uuid,
    action: &str,
    since: DateTime<Utc>,
) -> AppResult<Option<Uuid>> { ... }

/// Attaches token counts, the provider's cost and the prompt version to a usage row.
///
/// `usage_event_id: Option<Uuid>` because the generation job has no id: the row was
/// written by `spend`, and the job finds it with `latest_usage_event`. A `None` id is
/// a no-op rather than an error: a usage row that cannot be found is a reporting gap,
/// not a reason to fail a job whose cards are already stored and paid for.
///
/// `prompt_version` is mandatory: every model call must be attributable to a prompt
/// version, or a quality regression cannot be traced (the roadmap's rule).
///
/// `in_tokens`/`out_tokens` are `Option<u32>`: a failed generation records no tokens
/// rather than a zero, because zero would read as "the model returned nothing" in a
/// cost report.
pub async fn record_usage(
    pool: &PgPool,
    usage_event_id: Option<Uuid>,
    in_tokens: Option<u32>,
    out_tokens: Option<u32>,
    provider_cost_usd: Option<f64>,
    prompt_version: &str,
) -> AppResult<()> { ... }
```

Update the chat route's call site (`crates/server/src/chat/routes.rs:253-262`, inside
`if let Some(reported) = &usage`) to the new signature: `Some(usage_event_id)`,
`Some(reported.in_tokens)`, `Some(reported.out_tokens)`, `reported.provider_cost_usd`,
`&prompt_version_for_stream`. The chat path must record exactly what it records today;
`crates/server/tests/chat.rs` covers it.

Then wire it up: `crates/server/src/jobs/handlers/mod.rs` gains `pub mod generate;`,
`worker.rs::dispatch` gains
`"generate_flashcards" => crate::jobs::handlers::generate::run(state, job).await,`,
`crates/server/src/lib.rs` gains `pub mod flashcards;`, and
`routes/mod.rs` adds `.merge(flashcards::routes())` to the authenticated group.

- [ ] **Step 7: Run the tests to verify they pass**

Run: `cargo test -p stvdy-server --test flashcards`
Expected: PASS (5 tests in this slice).

- [ ] **Step 8: Commit**

```bash
git add migrations/0021_flashcard_generation.sql crates/server/src crates/server/tests/flashcards.rs
git commit -m "feat(server): decks, strict-JSON generation, and a refund for a failed job"
```

---

## Task 5: Review replay, due cards, and the two-device gate

**Files:**
- Modify: `crates/server/src/flashcards/routes.rs` (the review path is in Task 4's file; this task completes and tests it)
- Test: `crates/server/tests/flashcards.rs` (extend)

**Interfaces:**
- Consumes: everything Task 4 produced, plus `stvdy_core::sm2::{replay, Rating, Review}`.
- Produces: nothing new for later tasks; this is the exit gate.

- [ ] **Step 1: Write the failing test**

Append to `crates/server/tests/flashcards.rs`:

```rust
/// The exit gate: two clients reviewing one card produce the state a local replay
/// of the same rows produces, whatever order the rows arrived in.
#[sqlx::test(migrations = "../../migrations")]
async fn reviews_from_two_devices_replay_to_identical_state(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Apa itu TCP?", "Protokol transport.", 1)]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "two@example.test").await;
    let (workspace_id, source_id) = workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(&app, "GET", &format!("/decks/{deck_id}"), &token, serde_json::Value::Null).await;
    let card_id = deck["cards"][0]["id"].as_str().unwrap().to_string();

    // Device A grades `good`, then device B grades `easy` ten minutes later.
    let (status, first) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        serde_json::json!({ "rating": "good", "reviewed_at": "2026-10-11T09:00:00Z" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["applied"], true);

    let (status, second) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        serde_json::json!({ "rating": "easy", "reviewed_at": "2026-10-11T09:10:00Z" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");

    // Replay the same two rows locally: the state must match to the day.
    let user_id: uuid::Uuid =
        sqlx::query_scalar("select id from users where email = 'two@example.test'")
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let created_at: chrono::DateTime<chrono::Utc> =
        sqlx::query_scalar("select created_at from flashcards where id = $1::uuid")
            .bind(&card_id)
            .fetch_one(&harness.state.pool)
            .await
            .unwrap();
    let local = stvdy_core::sm2::replay(
        created_at,
        &[
            stvdy_core::sm2::Review {
                id: uuid::Uuid::from_u128(1),
                rating: stvdy_core::sm2::Rating::Good,
                reviewed_at: "2026-10-11T09:00:00Z".parse().unwrap(),
            },
            stvdy_core::sm2::Review {
                id: uuid::Uuid::from_u128(2),
                rating: stvdy_core::sm2::Rating::Easy,
                reviewed_at: "2026-10-11T09:10:00Z".parse().unwrap(),
            },
        ],
    );

    let stored: (f32, i32, chrono::DateTime<chrono::Utc>) = (
        second["card"]["ease"].as_f64().unwrap() as f32,
        second["card"]["interval_days"].as_i64().unwrap() as i32,
        second["card"]["due_at"].as_str().unwrap().parse().unwrap(),
    );
    assert!((stored.0 - local.ease).abs() < 1e-5, "{stored:?} vs {local:?}");
    assert_eq!(stored.1, local.interval_days);
    assert_eq!(stored.2, local.due_at);
    let _ = user_id;
}

/// A client that retries a review it already sent must not move the card twice.
///
/// This is what makes an offline outbox safe: the review id is the client's, and a
/// duplicate is ignored rather than applied. Phase 8's push replay depends on the
/// same rule.
#[sqlx::test(migrations = "../../migrations")]
async fn a_replayed_review_id_applies_once(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Apa itu TCP?", "Protokol.", 1)]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "replay@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(&app, "GET", &format!("/decks/{deck_id}"), &token, serde_json::Value::Null).await;
    let card_id = deck["cards"][0]["id"].as_str().unwrap().to_string();

    let review_id = uuid::Uuid::new_v4().to_string();
    let body = serde_json::json!({
        "id": review_id,
        "rating": "good",
        "reviewed_at": "2026-10-11T09:00:00Z",
    });

    let (status, first) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        body.clone(),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{first}");
    assert_eq!(first["applied"], true, "{first}");
    let first_due = first["card"]["due_at"].clone();

    let (status, second) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        body,
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{second}");
    assert_eq!(second["applied"], false, "the replay must not apply: {second}");
    assert_eq!(second["card"]["due_at"], first_due, "and must not move the card");

    // The row count is the real assertion: an implementation that inserted and
    // replayed would report `applied: true` above, but one that inserted without
    // replaying would leave two rows behind.
    let reviews: i64 = sqlx::query_scalar(
        "select count(*)::bigint from flashcard_reviews where flashcard_id = $1::uuid",
    )
    .bind(&card_id)
    .fetch_one(&harness.state.pool)
    .await
    .unwrap();
    assert_eq!(reviews, 1);
}

/// The study queue is the cards that are due, and a card graded `good` is not due
/// again for a day.
#[sqlx::test(migrations = "../../migrations")]
async fn the_due_queue_excludes_a_card_scheduled_for_later(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Apa itu TCP?", "Protokol.", 1)]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();
    let token = signed_up(&app, &harness, "due@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &token, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &token, &source_id, 1).await;
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    // A fresh card is due at the injected clock's instant, which is why
    // `insert_cards` must bind `state.clock.now()` and not the database's `now()`.
    let (status, due) = json(&app, "GET", &format!("/decks/{deck_id}/due"), &token, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::OK, "{due}");
    assert_eq!(due["cards"].as_array().unwrap().len(), 1, "{due}");
    let card_id = due["cards"][0]["id"].as_str().unwrap().to_string();

    // Graded `good` on the first repetition: one day, so tomorrow.
    let (status, reviewed) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &token,
        serde_json::json!({ "rating": "good", "reviewed_at": "2026-10-11T09:00:00Z" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK, "{reviewed}");
    assert_eq!(reviewed["card"]["interval_days"], 1, "{reviewed}");

    let (_, after) = json(&app, "GET", &format!("/decks/{deck_id}/due"), &token, serde_json::Value::Null).await;
    assert!(
        after["cards"].as_array().unwrap().is_empty(),
        "a card due tomorrow is not in today's queue: {after}"
    );

    // And it is still in the deck, with its new schedule: leaving the queue is not
    // the same as being deleted.
    let (_, deck) = json(&app, "GET", &format!("/decks/{deck_id}"), &token, serde_json::Value::Null).await;
    assert_eq!(deck["cards"].as_array().unwrap().len(), 1, "{deck}");
    assert_eq!(deck["cards"][0]["due_at"], reviewed["card"]["due_at"]);
}

/// Ownership is enforced on the card, not only on the deck: a card id from another
/// account is a 404, and so is that account's deck.
#[sqlx::test(migrations = "../../migrations")]
async fn another_users_card_is_not_found(pool: PgPool) {
    let harness = TestState::new(pool);
    harness.seed_plans().await.unwrap();
    let llm = Arc::new(ScriptedLlm::new(vec![reply("Apa itu TCP?", "Protokol.", 1)]));
    let harness = harness.with_llm(llm);
    let app = stvdy_server::routes::router(harness.state.clone()).unwrap();

    let owner = signed_up(&app, &harness, "owner@example.test").await;
    let (workspace_id, source_id) =
        workspace_with_text(&app, &harness, &owner, "Jaringan", "Modul 3", INDONESIAN).await;
    let (_, body) = deck_created(&app, &workspace_id, &owner, &source_id, 1).await;
    let deck_id = body["deck"]["id"].as_str().unwrap().to_string();
    drain_jobs(&harness).await;

    let (_, deck) = json(&app, "GET", &format!("/decks/{deck_id}"), &owner, serde_json::Value::Null).await;
    let card_id = deck["cards"][0]["id"].as_str().unwrap().to_string();

    let stranger = signed_up(&app, &harness, "stranger@example.test").await;

    let (status, _) = json(&app, "GET", &format!("/decks/{deck_id}"), &stranger, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another account's deck");
    let (status, _) = json(&app, "GET", &format!("/decks/{deck_id}/due"), &stranger, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another account's due queue");
    let (status, _) = json(
        &app,
        "POST",
        &format!("/flashcards/{card_id}/review"),
        &stranger,
        serde_json::json!({ "rating": "good" }),
    )
    .await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another account's card");
    let (status, _) = json(&app, "GET", &format!("/workspaces/{workspace_id}/decks"), &stranger, serde_json::Value::Null).await;
    assert_eq!(status, StatusCode::NOT_FOUND, "another account's workspace");
}

// The test clock is `FixedClock::at(Utc::now())` (`crates/server/src/testing.rs`),
// not a date in the past, so every review timestamp must be derived from
// `harness.clock.now()`: an absolute literal would make the gate fail on a machine
// whose clock is behind, and the route refuses a `reviewed_at` more than 24 hours
// ahead of the clock. Truncate to whole seconds, because `timestamptz` has
// microsecond resolution and rounds.
```

- [ ] **Step 2: Run the tests to verify they fail**

Run: `cargo test -p stvdy-server --test flashcards`
Expected: the review tests FAIL until the review path from Task 4 is complete;
if Task 4 already implemented it, they pass and this step is the confirmation that
the gate is real rather than assumed.

- [ ] **Step 3: Fix whatever the tests surface**

The likely defects, and what each means:
- A `due_at` mismatch means `replay` used `now()` somewhere instead of
  `reviewed_at`. The clock must not enter `crates/core/src/sm2.rs` at all.
- An `interval_days` mismatch after the second review means the replay started from
  the *stored* state rather than from `CardState::new(created_at)`. Decision 1.
- `applied: true` on the duplicate id means the `on conflict (id) do nothing` is
  missing or `rows_affected()` is being read after the `update`.

- [ ] **Step 4: Run the whole suite**

Run: `cargo test -p stvdy-core && cargo test -p stvdy-server`
Expected: PASS, with the pre-existing 242 tests unchanged.

- [ ] **Step 5: Commit**

```bash
git add crates/server/src/flashcards crates/server/tests/flashcards.rs
git commit -m "test(server): the two-device replay gate, duplicate ids, and the due queue"
```

---

## Task 6: The SPA — Studio panel, study screen, due computation

**Files:**
- Create: `app/src/api/queries/flashcards.ts`
- Create: `app/src/features/studio/FlashcardsPanel.tsx`
- Create: `app/src/features/study/{StudyScreen.tsx,due.ts,due.test.ts}`
- Create: `app/src/features/workspaces/SourcePicker.tsx` (extracted)
- Modify: `app/src/features/chat/ChatScreen.tsx`, `app/src/features/workspaces/WorkspaceScreen.tsx`,
  `app/src/router.tsx`

**Interfaces:**
- Consumes: `api.get/post`, `ApiError`/`asApiError`, `States.tsx`, `Field.tsx`,
  `Citation`, `useSources`, `useChunk`.
- Produces: `flashcardKeys`, `useDecks`, `useDeck`, `useCreateDeck`, `useDueCards`,
  `useReviewCard`, `dueCards`, `SourcePicker`.

- [ ] **Step 1: Write the failing test for the due queue**

`app/src/features/study/due.test.ts`:

```ts
/**
 * The due queue is computed on the client from the cards the server sent, because
 * the study screen must not refetch between reviews: a refetch mid-session would
 * reshuffle the queue under the student's finger. The order is therefore part of
 * the contract, not an implementation detail.
 */
import { describe, expect, it } from 'vitest'

import { dueCards, type StudyCard } from './due'

function card(id: string, dueAt: string): StudyCard {
  return { id, front: `f-${id}`, back: `b-${id}`, chunk_id: null, due_at: dueAt, ease: 2.5, interval_days: 0 }
}

describe('dueCards', () => {
  it('keeps only cards whose due_at has arrived', () => {
    const now = new Date('2026-10-11T09:00:00Z')
    const queue = dueCards(
      [card('a', '2026-10-11T08:00:00Z'), card('b', '2026-10-12T09:00:00Z')],
      now,
    )
    expect(queue.map((c) => c.id)).toEqual(['a'])
  })

  it('orders by due_at and then by id, so a refetch cannot reshuffle it', () => {
    const now = new Date('2026-10-11T09:00:00Z')
    const queue = dueCards(
      [
        card('c', '2026-10-10T09:00:00Z'),
        card('a', '2026-10-10T09:00:00Z'),
        card('b', '2026-10-09T09:00:00Z'),
      ],
      now,
    )
    expect(queue.map((c) => c.id)).toEqual(['b', 'a', 'c'])
  })

  it('is empty for an empty deck rather than throwing', () => {
    expect(dueCards([], new Date())).toEqual([])
  })
})
```

- [ ] **Step 2: Run it to verify it fails**

Run: `npm --prefix app test -- due`
Expected: FAIL, `Cannot find module './due'`.

- [ ] **Step 3: Implement the query module and the due filter**

`app/src/api/queries/flashcards.ts`:

```ts
import { useMutation, useQuery, useQueryClient } from '@tanstack/react-query'

import { api } from '../client'

/**
 * Decks, cards and reviews.
 *
 * A deck is generated asynchronously, so the deck query polls while its status is
 * `generating` — the same shape `useSourceStatus` uses for ingestion, because it
 * is the same problem and a second convention would be a second thing to learn.
 */

export type DeckStatus = 'generating' | 'ready' | 'failed'
export type Rating = 'again' | 'hard' | 'good' | 'easy'

export interface Deck {
  id: string
  workspace_id: string
  title: string
  source_ids: string[]
  status: DeckStatus
  error: string | null
  requested_count: number
  card_count: number
  created_at: string
}

export interface Flashcard {
  id: string
  deck_id: string
  front: string
  back: string
  /** The chunk this card came from, or null when the model's label was not sent. */
  chunk_id: string | null
  ease: number
  interval_days: number
  due_at: string
}

export const flashcardKeys = {
  decks: (workspaceId: string) => ['workspaces', workspaceId, 'decks'] as const,
  deck: (deckId: string) => ['decks', deckId] as const,
  due: (deckId: string) => ['decks', deckId, 'due'] as const,
}

export function useDecks(workspaceId: string) {
  return useQuery({
    queryKey: flashcardKeys.decks(workspaceId),
    queryFn: async ({ signal }) => {
      const body = await api.get<{ decks: Deck[] }>(`/workspaces/${workspaceId}/decks`, { signal })
      return body.decks
    },
    enabled: workspaceId.length > 0,
    // A deck in flight is the only reason to poll; once every deck has settled the
    // interval is off, so a parked tab stops asking.
    refetchInterval: (query) =>
      (query.state.data ?? []).some((deck) => deck.status === 'generating') ? 1500 : false,
  })
}

export function useDeck(deckId: string) {
  return useQuery({
    queryKey: flashcardKeys.deck(deckId),
    queryFn: async ({ signal }) =>
      api.get<{ deck: Deck; cards: Flashcard[] }>(`/decks/${deckId}`, { signal }),
    enabled: deckId.length > 0,
    refetchInterval: (query) =>
      query.state.data?.deck.status === 'generating' ? 1500 : false,
  })
}

export interface CreateDeckInput {
  title: string
  sourceIds: string[]
  count: number
}

export function useCreateDeck(workspaceId: string) {
  const client = useQueryClient()
  return useMutation({
    mutationFn: async (input: CreateDeckInput) =>
      api.post<{ deck: Deck }>(`/workspaces/${workspaceId}/decks`, {
        title: input.title,
        source_ids: input.sourceIds,
        count: input.count,
      }),
    onSuccess: () => client.invalidateQueries({ queryKey: flashcardKeys.decks(workspaceId) }),
  })
}

export function useDueCards(deckId: string) {
  return useQuery({
    queryKey: flashcardKeys.due(deckId),
    queryFn: async ({ signal }) =>
      api.get<{ cards: Flashcard[]; now: string }>(`/decks/${deckId}/due?limit=100`, { signal }),
    enabled: deckId.length > 0,
  })
}

export interface ReviewInput {
  rating: Rating
  /** Client-generated, so a retry after a lost response cannot apply twice. */
  id: string
  reviewedAt: string
}

export function useReviewCard(deckId: string) {
  return useMutation({
    mutationFn: async ({ cardId, input }: { cardId: string; input: ReviewInput }) =>
      api.post<{ applied: boolean; card: { id: string; ease: number; interval_days: number; due_at: string } }>(
        `/flashcards/${cardId}/review`,
        { id: input.id, rating: input.rating, reviewed_at: input.reviewedAt },
      ),
    // The queue is local state during a session, so only the deck's own data is
    // invalidated: refetching the due list mid-review would reshuffle it.
    onSuccess: () => client.invalidateQueries({ queryKey: flashcardKeys.deck(deckId) }),
  })
}
```

`app/src/features/study/due.ts`:

```ts
/**
 * The study queue, computed on the client.
 *
 * The server's `GET /decks/:id/due` returns the queue once, at the start of a
 * session. From then on the screen owns it, because a refetch between reviews
 * would reshuffle the cards under the student's finger. The order therefore has to
 * be reproducible here: `due_at` ascending, ties broken by id.
 *
 * This is due-ness only. The card's `ease`/`interval_days`/`due_at` after a review
 * are the server's, computed by the one SM-2 implementation in
 * `crates/core/src/sm2.rs`; a second implementation in TypeScript would be a second
 * schedule, and two schedules is how two devices start disagreeing.
 */

export interface StudyCard {
  id: string
  front: string
  back: string
  chunk_id: string | null
  due_at: string
  ease: number
  interval_days: number
}

export function dueCards(cards: StudyCard[], now: Date): StudyCard[] {
  return cards
    .filter((card) => Date.parse(card.due_at) <= now.getTime())
    .sort((left, right) => {
      const byDue = Date.parse(left.due_at) - Date.parse(right.due_at)
      return byDue !== 0 ? byDue : left.id.localeCompare(right.id)
    })
}
```

- [ ] **Step 4: Run the tests to verify they pass**

Run: `npm --prefix app test -- due`
Expected: PASS (3 tests).

- [ ] **Step 5: Extract the source picker**

Move `SourcePicker` out of `app/src/features/chat/ChatScreen.tsx` into
`app/src/features/workspaces/SourcePicker.tsx`, adding two props so the Studio panel
can say what it is selecting for:

```ts
export function SourcePicker({
  workspaceId,
  sources,
  selected,
  onChange,
  disabled,
  legend = 'Answer from',
  emptyHint = 'No indexed sources yet. Upload one in the Sources panel and this box will use it.',
}: {
  workspaceId: string
  sources: { id: string; title: string }[]
  selected: string[]
  onChange: (next: string[]) => void
  disabled: boolean
  /** What the selection is for. Defaults to the chat screen's wording. */
  legend?: string
  /** Shown instead of the rows when there is nothing to select. */
  emptyHint?: string
})
```

Import it in `ChatScreen.tsx` and delete the private copy. The chat suite must stay
green (`app/src/features/chat/ChatScreen.test.tsx` if it exists; otherwise the
existing 138 tests).

- [ ] **Step 6: Build the Studio panel**

`app/src/features/studio/FlashcardsPanel.tsx`:

```tsx
/**
 * Studio → Flashcards.
 *
 * Generating is asynchronous and costs credits, so the panel says both things
 * plainly: the button names the cost before it is pressed, and a deck in flight
 * shows its status rather than a spinner that never resolves. The count is a
 * number input rather than a preset because the plan's monthly allowance is the
 * real limit, and the refusal comes from the server with the numbers in it.
 */
export function FlashcardsPanel({ workspaceId }: { workspaceId: string }) {
  const sources = useSources(workspaceId)
  const decks = useDecks(workspaceId)
  const create = useCreateDeck(workspaceId)
  const [selected, setSelected] = useState<string[]>([])
  const [title, setTitle] = useState('')
  const [count, setCount] = useState(10)

  const readySources = (sources.data ?? []).filter((source) => source.status === 'ready')
  const error = asApiError(create.error)
  const ready = title.trim().length > 0 && selected.length > 0 && !create.isPending

  return (
    <div className="flex flex-col gap-4 px-4 py-4">
      <form className="panel flex flex-col gap-3 px-3 py-3" onSubmit={...}>
        <Field label="Deck name" name="deck-title" value={title} hint="What this deck is for." onChange={...} />
        <SourcePicker
          workspaceId={workspaceId}
          sources={readySources}
          selected={selected}
          onChange={setSelected}
          disabled={create.isPending}
          legend="Generate from"
          emptyHint="No indexed sources yet. Add one in the Sources panel, then come back."
        />
        <label className="label" htmlFor="deck-count">How many cards</label>
        <input id="deck-count" className="field" type="number" min={1} max={50} value={count} onChange={...} />
        <p className="text-xs text-ink-muted">
          Costs 4 credits per 10 cards. The deck appears here while it is generated, usually
          within a minute.
        </p>
        <button type="submit" className="btn btn-primary self-start" disabled={!ready}>
          {create.isPending ? 'Starting' : 'Generate deck'}
        </button>
        {error !== null && <ApiErrorNotice error={error} noun="decks" />}
      </form>

      {decks.isPending && <SkeletonRows rows={2} label="Loading this workspace's decks" />}
      {decks.data?.length === 0 && (
        <EmptyState
          title="No decks yet"
          reason="Pick the sources above and generate one. Every card is linked to the passage it came from."
        />
      )}
      {decks.data !== undefined && decks.data.length > 0 && (
        <ul className="flex flex-col divide-y divide-rule border-y border-rule">
          {decks.data.map((deck) => <DeckRow key={deck.id} deck={deck} />)}
        </ul>
      )}
    </div>
  )
}
```

`DeckRow` renders the title, a status chip using the existing `chip`/`chip-working`/
`chip-ready`/`chip-failed` classes and the same word-carrying-the-state rule as
`SourceRow`, the card count, the failure text in a `note note-danger`, and for a
ready deck a `Study` link to `/study/${deck.id}`.

- [ ] **Step 7: Build the study screen**

`app/src/features/study/StudyScreen.tsx`:

```tsx
/**
 * Study mode: one card at a time, four grades.
 *
 * The queue is seeded from the server's due list and then owned locally, so
 * grading a card does not make the list jump. The card is a focusable region and
 * focus moves to it on advance, because the whole point of a keyboard shortcut is
 * that the student never has to find the card again.
 *
 * Swiping is an accelerator, never the only way: every grade has a button and a
 * number key, and the swipe threshold is on the container so a vertical scroll
 * still works (`touch-action: pan-y`).
 */
export function StudyScreen() {
  const { deckId = '' } = useParams()
  const deck = useDeck(deckId)
  const due = useDueCards(deckId)
  const review = useReviewCard(deckId)

  const [queue, setQueue] = useState<StudyCard[] | null>(null)
  const [index, setIndex] = useState(0)
  const [revealed, setRevealed] = useState(false)
  const cardRef = useRef<HTMLDivElement>(null)

  // Seeded once, when the due list first arrives. `null` rather than `[]` so an
  // empty deck and a not-yet-loaded deck are different states.
  useEffect(() => {
    if (queue === null && due.data !== undefined) {
      setQueue(dueCards(due.data.cards, new Date()))
    }
  }, [due.data, queue])

  const card = queue?.[index] ?? null
  ...
}
```

Behaviour, exactly:
- Reveal: a `Show answer` button and the `Space` key. The back, and the citation chip,
  appear only after reveal.
- **The source, when the card has one.** Two facts, both verified: the card carries
  only `chunk_id` (no `source_id`, no title, no page), and `CitationPanel` is private
  to `ChatScreen` (`app/src/features/chat/ChatScreen.tsx:245`) and takes a
  `ChatCitation`, which requires a `source_id` the card cannot fill.
  So do **not** reuse `CitationPanel` and do **not** try to render a `Citation`
  (its `CitationSource` is `{ type: 'chunk'; label: number; sourceTitle: string;
  page: number | null }` — no chunk id at all). Instead: when `card.chunk_id !== null`,
  render a `Show source` button that toggles an inline panel containing the chunk text
  from `useChunk(card.chunk_id)` and the source's title looked up in `useSources(workspaceId)`
  by `chunk.source_id`. Both queries already exist and are cached by the chat screen, so
  this costs no new API surface. When `chunk_id` is null, render nothing: a card whose
  label was dropped has no source to show, and saying so in the UI would be noise on a
  card the student can still study.
- Grade: four buttons — `Again` `1`, `Hard` `2`, `Good` `3`, `Easy` `4`. Each posts
  `{ id: crypto.randomUUID(), rating, reviewed_at: new Date().toISOString() }`.
- On success: `setIndex(index + 1)`, `setRevealed(false)`, focus `cardRef.current`.
- On failure: keep the card, keep the reveal, show `ApiErrorNotice` with a `Try again`
  action. The review id is regenerated on retry only if the failed request never
  reached the server (`error.code === 'network'`); otherwise the same id is resent,
  which is what makes the server's `applied: false` path a no-op instead of a double
  grade.
- Progress: `Card {index + 1} of {queue.length}` in a `role="status"` element, so a
  screen reader announces the advance.
- End: `EmptyState` — "Nothing due in this deck" with the deck's next `due_at` when
  there is one, and a link back to the workspace.
- Swipe: `onTouchStart`/`onTouchEnd` on the card region; a horizontal delta over 60px
  grades `Again` (left) or `Good` (right); vertical movement past 20px is ignored so
  scrolling wins.

Add the route in `app/src/router.tsx`, inside the `App` children (after
`{ path: 'workspaces/:workspaceId', ... }`), and add the import beside the others:

```tsx
import { StudyScreen } from './features/study/StudyScreen'
// ...
          { path: 'study/:deckId', element: <StudyScreen /> },
```

Note: `WorkspaceScreen`'s three-panel grid stays `lg:`-only, so the `study` route
renders outside it, full-width, on every form factor. That is deliberate: the study
screen is one card, and a card squeezed into the third panel of a desktop grid would
be worse than a dedicated screen.

- [ ] **Step 8: Wire the Studio panel in**

In `app/src/features/workspaces/WorkspaceScreen.tsx`, replace the Studio
`NotBuiltYet` with `<FlashcardsPanel workspaceId={workspaceId} />` and drop the now
unused import if `NotBuiltYet` has no other use in the file. Update the file's
header comment, which currently says Studio "says which phase brings it".

- [ ] **Step 9: Run the frontend suite and the build**

Run: `npm --prefix app test && npm --prefix app run build` (the repo's convention, per
`justfile`. There is no vitest config file to create: the config is inline in
`app/vite.config.ts`, `include: ['src/**/*.{test,spec}.{ts,tsx}']`.)
Expected: all tests pass (138 existing + the new ones) and the build is clean.

- [ ] **Step 10: Commit**

```bash
git add app/src
git commit -m "feat(app): the Studio flashcard panel and the swipe study screen"
```

---

## Task 7: Docs, the two roadmap gates, and the smoke script

**Files:**
- Modify: `docs/API.md`, `docs/ARCHITECTURE.md`, `docs/ASSUMPTIONS.md`
- Create: `scripts/smoke-phase4.sh`
- Test: the gates themselves

**Interfaces:**
- Consumes: everything above.

- [ ] **Step 1: `docs/API.md`**

Replace the `## Flashcards (Phase 4)` stub with the full section: request bodies,
response shapes, the four status codes that matter (201 with `status: "generating"`,
402 `insufficient_credits`, 422 `limit_reached`, 404 for another account's deck), the
cost rule (4 credits per 10 requested cards), and a note that
`GET /workspaces/:id/decks` is an addition beyond the spec's route list, with the
reason (A75).

- [ ] **Step 2: `docs/ARCHITECTURE.md`**

Add a section stating the two properties that make this phase work, because both are
the kind of thing a later refactor breaks silently:
- The card's schedule is derived from the review rows, never authored, so two devices
  converge by construction rather than by conflict resolution. Include the exact
  grade map and interval rule.
- A generation that fails for good refunds the spend, with the guard that makes it
  once-only.

- [ ] **Step 3: `docs/ASSUMPTIONS.md`**

The next free number is **A75** (`docs/ASSUMPTIONS.md` ends at A74). Add one row per
decision, in the table's existing `| id | decision | detail | why |` shape:
- A75: `GET /workspaces/:id/decks` is a deliberate addition to the spec's route list.
- A76: the grade→quality map and the interval/ease formula are the algorithm's
  contract, not a tuning knob (name the exact values).
- A77: `{{output_language}}` defaults to the material's language.
- A78: `MAX_REQUESTED_CARDS = 50`, `MAX_SOURCES = 20`, `MAX_CARDS_PER_REPLY = 60`,
  and why the credit cost is quoted from the requested count.
- A79: `limits::enforce_batch` and the local-month window for `flashcards_per_month`.
- A80: client-side SM-2 *replay* is deferred to Phase 8 (client-side *due* computation
  is not).
- A81: the generation refund, including the zeroing guard that makes it once-only.
- A82: the migration-numbering rule when Phase 3's `0020_search_cache.sql` lands first.
- A83: `plan_id` parsing moved to `PlanId::parse`, so one implementation reads a plan
  string.

- [ ] **Step 4: Roadmap gate 5 — accessibility, on the study screen**

Perform and record, in the plan and in the phase summary:
(a) keyboard-only walk: Tab reaches the reveal button and all four grade buttons, the
focus ring is visible on each, and `1`-`4` grade without touching the mouse;
(b) contrast in both themes for the card, the revealed answer, and the four grade
buttons — measure, do not assume;
(c) 44px minimum targets at phone width for the four grade buttons (they are the
smallest controls on the screen);
(d) no state conveyed by colour alone: the status chip carries a word, and a grade's
effect is announced by the progress line, not by a colour change;
(e) 200% text scale on a 390px viewport with no clipping or overlap.
State the check performed and the result. "Looks fine" is not a gate.

- [ ] **Step 5: Roadmap gate 6 — Indonesian end to end**

The server test from Task 4 covers it (`a_deck_generates_cards_linked_to_their_chunk`
uses an Indonesian source and asserts the prompt carried the label block and the
material-language default). Add the UI half: with the Indonesian fixture, generate a
deck from the Studio panel, open the study screen, and confirm the card text is
Indonesian and the citation chip opens the Indonesian chunk. Record the output.

- [ ] **Step 6: The smoke script**

`scripts/smoke-phase4.sh`, modelled on `scripts/smoke-phase2.sh`: start the server
against the dev database with the stub LLM, sign up, create a workspace, add an
Indonesian text source, create a deck, poll until `ready`, review the first card twice
with two different `reviewed_at` values, and print the card's
`ease`/`interval_days`/`due_at` beside a local replay of the same two rows. Print
`PASS` only when the two agree.

Three facts about the stub that this script depends on (`scripts/stub-llm.py`):
- It already serves a **non-streaming** request (`if not body.get("stream")` returns
  `{"choices":[{"message":{"content": Handler.answer}}]}`), which is the shape
  `generate_flashcards` uses. No stub change is needed.
- Its reply is the fixed `--answer` string, so start it with the cards JSON:
  `python3 scripts/stub-llm.py --port "$STUB_PORT" --log "$STUB_LOG" --answer '{"cards":[{"front":"Apa fungsi three-way handshake?","back":"Membuka koneksi TCP.","label":1}]}'`
- Its reply carries no `usage` object, so token counts and cost are absent on this
  path. The script must **not** assert on them: `record_usage` is a no-op when the
  row cannot be found, and a `None` cost is the honest record (A41). Assert the
  prompt version and the action instead, which are the fields the job controls.

- [ ] **Step 7: Run the full gate**

```bash
cargo fmt --check
cargo clippy --all-targets -- -D warnings
cargo test --workspace
npm --prefix app test
npm --prefix app run build
cargo sqlx prepare --workspace --check
bash scripts/smoke-phase4.sh
```

Expected: all clean. Quote the smoke output in the phase summary.

- [ ] **Step 8: Commit**

```bash
git add docs scripts/smoke-phase4.sh
git commit -m "docs(phase 4): the API, the derived-schedule property, and both gates"
```

---

## Phase 4 exit checklist

- [ ] `crates/core/src/sm2.rs` exists, is pure, and its vectors are the ones Phase 8
      will check the TypeScript replay against.
- [ ] A deck is generated from selected sources, strictly JSON, with one retry.
- [ ] Every card is linked to a chunk; a label that was never sent costs the link,
      not the card, and the drop is counted.
- [ ] A generation that fails for good refunds the credits exactly once.
- [ ] `flashcards_per_month` is enforced in the user's local month, via
      `limits::enforce_batch`, with no hardcoded number.
- [ ] Two devices' reviews replay to identical `ease`/`interval_days`/`due_at`, and a
      duplicate review id applies once.
- [ ] The study screen reviews a deck by keyboard and by swipe, and says what it
      costs before charging.
- [ ] Roadmap gate 5 (accessibility) performed and recorded.
- [ ] Roadmap gate 6 (Indonesian) performed and recorded.
- [ ] `docs/{API,ARCHITECTURE,ASSUMPTIONS}.md` updated in the same commit as the code.
- [ ] The full quality gate is green.

---

## Self-Review

**Spec coverage.** `docs/SPEC.md` § Flashcards asks for: generation from selected
sources (Task 4), strict JSON validated with serde and one retry on a parse failure
(Tasks 3 and 4), each card linked to its source chunk (Tasks 3 and 4), append-only
reviews with the server deriving ease/interval/due by replaying in `reviewed_at`
order with SM-2 (Tasks 1 and 5), a study endpoint returning due cards (Task 5), and
clients able to compute due cards locally from synced state (Task 6, due-ness only —
see decision 10). The roadmap's Phase 4 line adds "chunk-linked cards" and "swipe
study UI", both covered. The API list's four routes are implemented, plus
`GET /workspaces/:id/decks` (decision 9).

**Deliberate deferrals, each with its reason.** Client-side SM-2 *replay* waits for
Phase 8's SQLite cache and outbox (decision 10); there is no local store to replay
from, and the offline write path is what needs it. `GET /flashcards/due` across all
decks (for the Today screen's placeholder) belongs to Phase 9 with the rest of Today.
The full module sweep over every `prompts/*.md` file belongs to Phase 6 with the
modules it adds.

**Placeholders.** None. Every code step carries the code; the three elided tests in
Task 5 are named, described, and follow the full test printed above them.

**Type consistency.** `Rating`, `Review`, `CardState`, `replay`, `GeneratedCard`,
`ResolvedCard`, `ParseOutcome`, `GenerationError`, `parse_cards`, `resolve_cards`,
`is_retryable`, `ModuleId`, `compose_task`, `trim_base`, `enforce_batch`, `refund`,
`local_month_start`, `owned_name`, `base_vars`, `fail_and_refund`, `Deck`, `Flashcard`,
`StudyCard`, `dueCards`, `flashcardKeys` are each defined once and referenced by the
same name afterwards. `Composed`, `PromptChunk`, `ChatVars`, `ScriptedLlm`,
`ClaimedJob`, `Failure` and `Spent` keep their existing definitions.

**Known risks carried into execution.**
1. The migration number collides with Phase 3's `0020_search_cache.sql` if that lands
   first; the rename is stated in the file-structure section.
2. `credits::spend` and `record_usage` are the Phase 2 shim; Phase 7
   generalises them. The job's refund path is a second write to `credit_ledger` and
   Phase 7 must keep it working when the spend moves behind `check`.
3. The refund depends on `decks.credits_spent` being zeroed in the same statement as
   the status change, in **both** directions: the failure path zeroes it while marking
   the deck `failed`, and the success path zeroes it while claiming the deck `ready`.
   A refactor that splits either write, or drops the success-side zeroing, makes a
   delivered deck refundable.
4. `ScriptedLlm::new(vec![])` returns a provider error on the first call; the
   monthly-limit test relies on the limit refusing the request *before* any call, which
   is the property the test is actually asserting.
