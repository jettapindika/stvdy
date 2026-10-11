# Phase 3. Web search. Implementation Plan

> **For agentic workers:** REQUIRED SUB-SKILL: Use `superpowers:subagent-driven-development`
> or `superpowers:executing-plans` to implement this plan task-by-task. Steps use
> checkbox (`- [ ]`) syntax for tracking.
>
> Contracts are fixed in `docs/superpowers/plans/2026-10-11-study-os-master-roadmap.md`.
> This plan implements them; it never redefines them. Where this plan and the
> roadmap disagree, the roadmap wins and this file is the bug.

**Goal:** a chat turn with `use_web` on searches the web through a composite of
free tiers, cites results by URL **and DOI**, and costs one credit even when the
same question is asked twice. When the free tiers are all unavailable the answer
still arrives, grounded in the user's own sources.

**Exit gate (observable, from the roadmap):** the toggle on produces cited
results with URLs **and DOIs**; a repeated query bills once (the second is a cache
hit); with SearXNG stopped and both fallback keys unset the request still answers;
"save to workspace" creates a `kind=web` source.

---

## What already exists (checked, not assumed)

- `searxng/settings.yml` carries the **tuned** engine list from A3 (`brave`,
  `duckduckgo`, `qwant`, `startpage` disabled; `mojeek` explicitly re-enabled from
  upstream `inactive: true`; `mwmbl` on). The compose service and its init
  container exist from Phase 0.
- `citations::Citation::Web { url, title }` and the `[W1]` label syntax exist and
  are tested (`crates/core/src/citations.rs`, `tests/citations.rs`).
- `messages.citations` already documents the `{type:"web",url,title}` shape
  (`migrations/0008_messages.sql`).
- `sources.kind` already allows `web` (`migrations/0007`), and `sources` carries
  `url`, so "save to workspace" needs no schema change.
- **Nothing else.** No `traits/search.rs`, no `search/rank.rs`, no provider
  implementations, no `search_cache` table, and `usage_events` has no `cache_hit`
  column.

## Decisions this plan pins (so parallel work cannot diverge)

1. **The trait lands in `crates/core/src/traits/search.rs`** exactly as roadmap
   § 1 specifies:

```rust
#[derive(Debug, Clone, Copy, PartialEq, Eq, serde::Serialize, serde::Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SourceKind { Web, Academic }

#[derive(Debug, Clone, serde::Serialize, serde::Deserialize)]
pub struct SearchHit {
    pub title: String,
    pub url: String,
    pub snippet: String,
    /// Real only for the academic tier and Exa. Tavily and SearXNG returned null
    /// in 20 of 20 measured results (A3), so nothing may assume it is present.
    pub published_at: Option<DateTime<Utc>>,
    pub source_kind: SourceKind,
    pub doi: Option<String>,
    /// The tier that produced this hit, for ranking and for the admin view.
    pub provider: String,
    pub score: Option<f32>,
    pub license: Option<String>,
}

#[derive(Debug, thiserror::Error)]
pub enum SearchError {
    /// Transport or upstream failure. Retryable, and never fatal for the request:
    /// the composite records it and moves to the next tier.
    #[error("{0}")] Provider(String),
    /// The request itself is wrong (bad key, bad parameter). Also not fatal.
    #[error("{0}")] Invalid(String),
    /// The tier is configured but not reachable, e.g. SearXNG is down.
    #[error("{0}")] Unavailable(String),
}

#[async_trait::async_trait]
pub trait SearchProvider: Send + Sync {
    fn name(&self) -> &'static str;
    async fn search(&self, q: &str, max_results: u8) -> Result<Vec<SearchHit>, SearchError>;
}
```

2. **Ranking is pure and lives in `crates/core/src/search/rank.rs`.** Two
   functions, both deterministic:
   - `normalize_url(raw: &str) -> Option<String>`: lowercase the scheme and host,
     strip the fragment, strip tracking parameters (`utm_*`, `fbclid`, `gclid`),
     strip a trailing slash, keep the query otherwise. Returns `None` for a
     non-http(s) URL, so a `javascript:` or `mailto:` hit is dropped rather than
     ranked.
   - `rank(hits: Vec<SearchHit>) -> Vec<SearchHit>`: dedupe by normalized URL,
     then by DOI (a DOI match wins over a URL match, because the same paper is
     often reachable at two hosts), then sort by `(kind_priority: Academic > Web,
     has_doi, published_at desc with `None` last, tier order, score desc)`, with a
     final tie-break on normalized URL so the order is total.
   - `dedupe_key(query: &str, provider: &str, max_results: u8) -> String`: the
     cache key from A40, `sha256(normalized_query | provider | max_results)`, where
     `normalized_query` is lowercased, whitespace-collapsed and trimmed.

3. **A `search_cache` table and three `usage_events` columns, in one migration.**
   `migrations/0020_search_cache.sql`:
   - `search_cache(key text primary key, provider text not null, results jsonb not
     null, fetched_at timestamptz not null default now(), expires_at timestamptz not
     null)`, index on `expires_at` for the purge job.
   - `alter table usage_events add column cache_hit boolean not null default false`,
     `add column search_tier text`, `add column unresponsive_engines text[]`.
   The cache is **global, not per-user** (A40): a query string carries no user data,
   and per-user rows would multiply storage by the user count for no benefit.

4. **The composite is itself a `SearchProvider`** (roadmap § 1). `CompositeSearch`
   holds `tiers: Vec<Arc<dyn SearchProvider>>`, a pool, and the TTL. Its `search`:
   for each tier in order, compute the cache key, read the cache, and on a hit
   return the cached results (marking the outcome `cache_hit`); on a miss call the
   tier; if it returns a non-empty set, cache it and stop; if it returns empty or
   errors, record that tier as unresponsive and continue. Every tier's outcome is
   reported back to the caller so one `usage_events` row carries the tier that
   answered plus the ones that did not.

5. **Each provider is its own module and is skipped when unconfigured.** A tier
   whose env key is absent is not constructed at all, so `SEARCH_PROVIDER=tavily`
   with no `TAVILY_API_KEY` fails at boot with a message naming the key rather than
   at the first query with a 401.

6. **`SEARCH_PROVIDER` selects the composite or pins one tier** (A3):
   `composite` (default), `academic`, `searxng`, `exa`, `tavily`.

7. **`use_web` is un-refused in all three places at once.** The server stops
   rejecting it, the SPA's toggle becomes a real control, and the citation path
   already carries the `Web` variant. Shipping any one without the other two leaves
   the feature unreachable or dishonest, which is the defect A65 recorded.

8. **SearXNG cannot be exercised on this machine** (Docker is not running), and A3
   warns not to tune its engine list against a laptop. So the SearXNG tier is
   tested against a **stub HTTP server** in the smoke script, and the real engine
   list is only verified on the staging VPS. This is recorded, not glossed: a green
   local run says nothing about the engine list.

9. **The SSRF rule is a test, not a comment.** Every `PageFetcher` path passes
   `--block-private-networks` where the tool supports it, and the reqwest fallback
   resolves the host and refuses a private range before connecting. A URL pointing
   at `127.0.0.1`, `169.254.169.254` (cloud metadata) or `10.0.0.0/8` must be
   refused by the fallback, with a test per range.

---

## File structure (new this phase)

```
crates/core/src/
  traits/search.rs                       # SearchProvider, PageFetcher, DTOs
  search/{mod.rs,rank.rs,normalize.rs}   # pure ranking, dedupe, cache key
crates/core/tests/{search_rank.rs,search_normalize.rs}
crates/server/src/
  integrations/search/{mod.rs,academic.rs,searxng.rs,exa.rs,tavily.rs,composite.rs}
  integrations/fetch.rs                  # PageFetcher: Exa -> moli -> reqwest
  chat/web.rs                            # web context block + [Wn] labels
  sources/web.rs                         # save-to-workspace for a web result
crates/server/tests/{search.rs,web_citations.rs,ssrf.rs}
migrations/0020_search_cache.sql
app/src/features/chat/{WebToggle,WebCitation}.tsx
scripts/stub-searxng.py                  # speaks the SearXNG JSON API
scripts/smoke-phase3.sh
```

---

## Task 1: `crates/core`: the search trait, normalizer and ranker

**Files:** `src/traits/search.rs`, `src/search/{mod.rs,rank.rs,normalize.rs}`,
`src/traits/mod.rs`, `src/lib.rs`, `tests/{search_rank.rs,search_normalize.rs}`.

- [ ] **Step 1: The trait and DTOs**, verbatim from decision 1.
- [ ] **Step 2: `normalize.rs`.** `normalize_url`, `normalize_query`, and
  `dedupe_key`. Tracking parameters to strip: any `utm_*`, `fbclid`, `gclid`,
  `mc_cid`, `mc_eid`, `ref`, `source`. Lowercase scheme and host, keep path case
  (paths are case-sensitive), drop the fragment.
- [ ] **Step 3: `rank.rs`.** `rank(hits) -> Vec<SearchHit>` per decision 2.
- [ ] **Step 4: Tests.** Normalizer: `HTTP://Example.COM/a/?utm_source=x#frag` and
  `https://example.com/a/` collapse to one key; a `javascript:` URL is `None`; a
  DOI-only difference is not a URL difference. Ranker: an Academic hit outranks a
  Web hit; a DOI duplicate collapses into one entry and the survivor keeps the DOI;
  two hits with equal everything sort by URL so the output is identical across
  runs; a hit with `published_at` sorts above one without; `None` published dates
  sort last rather than first (a common bug: `None` as the minimum).

**Acceptance:** `cargo test -p stvdy-core` green; the module has no IO and no clock.

## Task 2: Migration `0020_search_cache.sql`

**Files:** `migrations/0020_search_cache.sql`.

- [ ] **Step 1: Write it** per decision 3, with the reasoning in comments: the cache
  is global because a query carries no user data (A40); `unresponsive_engines` is on
  `usage_events` so silent degradation of the free path shows up in the margin
  report instead of being discovered by a user.
- [ ] **Step 2: Apply** against the existing dev database (not only a fresh one,
  because append-only means the migration must work on a populated database).
- [ ] **Step 3: Add both names to `crates/server/tests/schema.rs`'s known set** and
  record the table in `docs/ASSUMPTIONS.md` § Schema deviations, which is that
  test's contract.

**Acceptance:** `sqlx migrate run` applies on the existing database; the schema test
passes.

## Task 3: Server: the four providers

**Files:** `src/integrations/search/*.rs`, `Cargo.toml`.

- [ ] **Step 1: `academic.rs`.** One provider per API (OpenAlex, Crossref, arXiv,
  DOAJ, ERIC, PubMed), each keyless or free-key, each mapping its response to
  `SearchHit` with `source_kind: Academic` and a real `published_at` and `doi`
  where the API gives them. They are tiers 1a..1f, tried in that order.
- [ ] **Step 2: `searxng.rs`.** `GET {SEARXNG_URL}/search?q=&format=json`, mapping
  `results[]` to `SearchHit`. The response's `unresponsive_engines` is **captured
  and surfaced**, not dropped: it is how A3's engine-list health reaches the admin
  report. An empty `results` with a non-empty `unresponsive_engines` is `Ok(vec![])`
  plus a recorded list, not an error.
- [ ] **Step 3: `exa.rs`** (`x-api-key`) and **`tavily.rs`** (`Authorization: Bearer`).
  Both map to `SearchHit`; Exa carries a published date, Tavily does not.
- [ ] **Step 4: `composite.rs`** per decision 4, with the cache read/write and the
  outcome record. `max_results` defaults to 8.
- [ ] **Step 5: Tests** against a stub HTTP server: a tier returning results stops
  the walk; a tier erroring is recorded and the next tier is tried; all tiers
  failing yields an empty vec and an outcome naming every tier; a second identical
  query is a cache hit and does not call the stub again (asserted by request count).

**Acceptance:** the suite passes and the request count assertion proves the cache.

## Task 4: `PageFetcher` and save-to-workspace

**Files:** `src/integrations/fetch.rs`, `src/sources/web.rs`, `tests/ssrf.rs`.

- [ ] **Step 1: The three-step fetcher** (Exa `/contents` → `moli` → reqwest +
  readability), each behind the same trait, each carrying `--block-private-networks`
  or the equivalent check.
- [ ] **Step 2: The SSRF guard** for the reqwest path: resolve the host, refuse
  loopback, private, link-local and metadata ranges, and refuse a redirect that
  lands in one. Tested per range.
- [ ] **Step 3: `POST /workspaces/:id/sources` gains a `kind=web` shape** taking
  `{url}`: fetch, store the extracted text as the object, insert the source with
  `kind='web'` and `url` set, and enqueue the normal ingest job.
- [ ] **Step 4: Tests.** A fetch of a private URL is refused with a clear error and
  no request made; a saved web result appears in the source list with its URL; the
  page text becomes chunks.

## Task 5: Chat integration: the web context block and `[Wn]` citations

**Files:** `src/chat/{routes.rs,web.rs,prompt.rs}`, `src/credits.rs`.

- [ ] **Step 1: Un-refuse `use_web`** (decision 7) and pass the flag through.
- [ ] **Step 2: `web.rs`.** Runs the composite, cleans and truncates each snippet,
  builds `PromptWebHit`s labelled `[W1]`, `[W2]`, and returns the labels for the
  citation resolver. Truncation is per hit and bounded, because a page's snippet
  can be large and the context has a budget.
- [ ] **Step 3: Credits.** `web_search` is charged **once per search**, and a cache
  hit charges **nothing** while still writing a `usage_events` row with
  `cache_hit = true` (A40). The existing spend path is reused; the cache-hit path
  writes the row without a ledger entry.
- [ ] **Step 4: Tests.** A web answer cites `[W1]` and resolves to `{type:"web",
  url, title}`; the same query twice bills one credit and the second row says
  `cache_hit`; an unknown `[W9]` is dropped on both the streamed and the stored
  path (the Phase 2 property, which must not regress now that a second label
  family exists).

## Task 6: Acceptance: the free path degrades without failing

**Files:** `crates/server/tests/search.rs`, `scripts/smoke-phase3.sh`,
`scripts/stub-searxng.py`.

- [ ] **Step 1: The roadmap's acceptance case.** With the SearXNG tier pointed at a
  closed port and both fallback keys unset, a `use_web` chat still answers: the
  academic tier is consulted first, so it answers from there; if it returns
  nothing, the turn proceeds **without** web context rather than erroring.
- [ ] **Step 2: The smoke run** drives the real server with the stub SearXNG (this
  machine cannot run the real one, decision 8) and asserts: a URL **and** a DOI in
  the citations, the second identical query is a cache hit, and the toggle-off path
  is unchanged.
- [ ] **Step 3: Record what was not verified**: the real SearXNG engine list, which
  needs the staging VPS. A3's measured baseline (1 of 8 engines on a laptop, 3 on
  the production host) is the reason not to tune it here.

## Task 7: SPA: the toggle and web citations

**Files:** `app/src/features/chat/*`, `app/src/api/queries/chat.ts`.

- [ ] **Step 1: The toggle becomes real**: enabled, remembered per workspace, and
  sent as `use_web`. Its copy says what it does now, replacing the "arrives in
  Phase 3" note.
- [ ] **Step 2: Web citations render** with their host and title, and open in the
  system browser through the platform adapter (never an in-app webview).
- [ ] **Step 3: Gate 5 on the changed screen**, stated with measured results.
- [ ] **Step 4: Gate 6 still holds** with web context present: an Indonesian query
  with `use_web` on still retrieves and cites the Indonesian source.

## Task 8: Docs and the phase gate

- [ ] **Step 1: `docs/API.md`**: the `use_web` behaviour, the `kind=web` upload
  shape, and the web citation shape.
- [ ] **Step 2: `docs/ARCHITECTURE.md`**: the composite's tier order and why,
  the ranking rules, the cache's global scope, and the SSRF guard.
- [ ] **Step 3: `docs/ASSUMPTIONS.md`**: A65 superseded (the refusal is lifted),
  the new table, the SSRF policy, and the SearXNG-verification gap.
- [ ] **Step 4: Run the gate**: fmt, clippy, tests, web build, shell, the desktop
  bundle, `sqlx prepare --check`, and the smoke.
- [ ] **Step 5: Phase summary** with gates 5 and 6 stated as performed checks.

## Phase 3 exit checklist

- [ ] fmt, clippy, `cargo test --workspace`, `npm test`, `npm run build`,
  `tauri build`, `sqlx prepare --check` all green.
- [ ] A `use_web` turn streams cited results with a **URL and a DOI**.
- [ ] The same query twice bills **one** credit; the second row says `cache_hit`.
- [ ] SearXNG down and both fallback keys unset: the request still answers.
- [ ] "Save to workspace" creates a `kind=web` source that ingests to `ready`.
- [ ] A private-address URL is refused by the fetcher, per range.
- [ ] An unknown `[W9]` is dropped from the streamed text as well as the stored copy.
- [ ] Gate 6 still passes with web context in the prompt.
- [ ] What could not be verified here (the real SearXNG engine list) is named.
- [ ] Phase summary posted; wait for "continue".

## Execution notes (Phase 3, as built)

1. **The cache was read per tier inside the walk, and that double-billed.** Which
   free academic API answers is not stable: they return empty under load without
   erroring, so two identical queries can be answered by two different tiers.
   Reading only the tier currently being tried then misses the entry the previous
   answer wrote, calls the network again, and charges a second search for a
   question already answered. The read now happens for every tier before any tier
   is called (A75). Found by the smoke run, whose query OpenAlex and Crossref
   answered on consecutive identical calls.
2. **The DOI had nowhere to travel.** `SearchHit` carried one, but neither
   `PromptWebHit`, nor `LabeledWeb`, nor `Citation::Web` had a field for it, so the
   gate's "URLs and DOIs" would have been URLs only. Threaded through all three,
   plus the prompt block, because a DOI the model never sees is one it cannot cite.
3. **`used_web` meant "the toggle was on" rather than "this turn used web".** A
   toggle-on turn where every tier returned nothing stored `true`, which is a lie
   about where the answer came from. It is now `!web.is_empty()`.
4. **The spend covered the chat charge but the usage event reported the total**, so
   a web turn debited one credit and told the client three. One spend now covers
   both charges, in one lock and one transaction.
5. **`SEARCH_PROVIDER` in an untracked `.env` broke every suite.** The documented
   acceptance command sources `.env`, and a stale `SEARCH_PROVIDER=tavily` with no
   key made `search_config` fail at boot for every integration test in the crate.
   `testing::config` now pins the search keys, because `from_env_at` falls back to
   the process environment for anything not in its override list; that is the same
   class of surprise the Phase 0 note recorded about global environment state.
6. **Three academic API mappings were wrong and the live APIs showed it.** PubMed's
   `articleids` entries carry `value`, not the documented `idval`, so every PubMed
   DOI would have been `None`. arXiv reports a rejected query as a well-formed feed
   whose entry is titled "Error", which would have become a citable hit. ERIC
   silently ignores an unquoted multi-word query and returns the whole corpus,
   which would have filled the prompt with the first N records of everything.
   Checking against the real APIs is what found these; they are invisible in a
   fixture written from the same assumption as the code.
7. **The reqwest feature set needed `query`**, which the tiers use to build
   URL-encoded query strings.

### Phase 3 verification (as executed)

| Gate | Result |
|---|---|
| `cargo fmt --all -- --check` | clean |
| `cargo clippy --workspace --all-targets -- -D warnings` | clean |
| `cargo test --workspace` | **317 passing**, 0 failing |
| `npm --prefix app test` | **144 passing** |
| `npm --prefix app run build` | clean |
| `src-tauri` tests + `tauri build --no-bundle` | green |
| `cargo sqlx prepare --workspace --check` (`SQLX_OFFLINE=true`) | exits 0 |
| Migration `0020` | applied to the **existing** dev database, and idempotent on a re-run |
| End-to-end smoke (`scripts/smoke-phase3.sh`) | PASS, quoted below |

The smoke run's own output, which is the acceptance evidence:

```
1 web citation(s), 1 with a DOI
  https://doi.org/10.5281/zenodo.19225200  doi=10.5281/zenodo.19225200
first turn spent 3, second (cached) spent 1
search rows (cache_hit|tier): ['false|openalex', 'true|openalex']
ledger 56 == reported 56
2 result(s), 2 unresponsive engine(s)
search rows unchanged at 15
prompt carries the Indonesian chunk labelled [1] and a web result labelled [W1]
```

**The DOI is real**, from OpenAlex, not from a fixture: two separate runs returned
`10.5281/zenodo.19225200` and `10.21203/rs.3.rs-10916252/v1`.

### What this phase did NOT verify, and why

- **The real SearXNG engine list.** Docker is not running on this machine, and A3
  measured that a laptop answers with 1 of 8 engines against 3 on the production
  host. Verifying or tuning the list from here would measure the wrong sample, so
  the tier is exercised against `scripts/stub-searxng.py` and the list is verified
  on the staging VPS (A45). The smoke run prints this at the end.
- **Exa and Tavily.** No keys. Their request bodies and response mappings are
  documentation-only, and their `SearchHit` mappings are covered by unit tests
  against recorded shapes rather than against the live services.
- **`moli`.** Not installed here, so the chain is Exa (absent) then reqwest; the
  `--block-private-networks` invocation is asserted by construction, not by running
  it.

### Review round: four defects found after the first Phase 3 summary

8. **`web_searches_per_day` was configured but never read.** Three on free, 30 on
   pro, 100 on power, and nothing enforced any of it. Exactly the shape Phase 1
   shipped for `sources_total`, and the same fix: the cap now lives inside the
   spend's transaction, alongside the chat cap, and counts **uncached** searches
   only, because a cache hit costs no credit and must not consume the daily
   allowance either. Both caps are checked in one transaction, which is what makes
   a refused turn charge nothing: two separate spends would charge for the chat and
   then refuse the search, leaving a billed turn with no web context (A78).
9. **A search hit's URL could reach a citation without ever being checked.** The
   citation path does not go through the fetcher: a hit's URL is written into
   `Citation::Web` and rendered as a link the shell opens in the user's browser,
   where their own credentials may live. So a hostile or compromised tier could
   return `http://169.254.169.254/latest/meta-data/` or this server's own loopback
   address and have it offered as a source. Unusable URLs are now dropped at the
   tier boundary, before ranking and before the cache write, so an unusable URL can
   be neither cited nor stored (A79). The suite caught two gaps while pinning it:
   `localhost` is a name rather than a literal so an address-only check missed it,
   and `host_str()` keeps an IPv6 literal's brackets so `[::1]` failed to parse as
   an address and the check silently did not run.
10. **`used_web` was one value for two different facts.** It was written to the
    user's row, the assistant's row and the `done` event from the same expression.
    The user's row should record what the turn asked for, and the assistant's row
    and the event should record whether web context actually reached the prompt;
    they differ in exactly the case where the toggle was on and every tier returned
    nothing, and sharing one value misattributes the answer on that turn (A80).
11. **A redirect was not re-checked for its scheme.** The comment claimed the next
    iteration re-checked "the scheme and the addresses" and only the addresses were
    re-checked, so a redirect to a non-http scheme failed later inside `send()` as a
    provider error, which reads like an outage rather than a refusal. `parse_target`
    now runs on every joined redirect target as well as at the top.

### Roadmap gate 6, with web context present: PASS

The Indonesian query, with `use_web: true`, still has its chunk in the prompt
labelled `[1]` alongside the web result labelled `[W1]`. Asserted on the recorded
prompt rather than on the answer, because whether the model *chooses* to cite the
chunk is the model's behaviour while whether the chunk was *available* is the
server's, and that is what the gate is about. The chunk's own citation resolution
with web context present is covered by `crates/server/tests/indonesian.rs`.

### Roadmap gate 5: accessibility on the changed screen

The changed controls are the web toggle and the web citation chip, both on the chat
screen. The SPA's suite asserts the toggle is a real, keyboard-operable control
(it was a permanently disabled checkbox) and that a web citation renders as a link
with a distinguishable focus state and a target at least 44px at phone width, with
its on/off state carried by a label rather than by colour alone. The DOI line is
hidden below the phone breakpoint where the chip already carries a title and a
host, so it cannot push the target below 44px.

## Self-Review

- **Spec coverage.** SPEC § Retrieval and chat's `use_web` paragraph and the
  roadmap's Phase 3 detail are covered: the composite with the four tier groups,
  the Postgres cache with `cache_hit` accounting, ranking and URL/DOI dedupe as a
  pure function, the cleaned/truncated web block, URL and DOI citations,
  `PageFetcher` for save-as-source, and the acceptance case.
- **Deliberate boundaries.** The admin margin report that *consumes*
  `unresponsive_engines` is Phase 13's; Phase 3 records the data. Rate limiting per
  provider is Phase 7's. `watch`-style freshness is not needed: the TTL is the
  freshness mechanism.
- **Risk.** The largest is that the free tiers are all unverifiable here (no
  Docker, no keys), so the local evidence is a stub. Mitigation: the stub speaks the
  real wire format, the real engine list is explicitly recorded as unverified, and
  the acceptance case is the *degraded* path, which is the one that must not fail.
- **Type consistency.** `SearchProvider`, `SearchHit`, `SourceKind`, `SearchError`,
  `SearchOutcome`, `PageFetcher`, `FetchedPage` are each defined once and
  referenced by name thereafter.
