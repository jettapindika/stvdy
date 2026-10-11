-- Search result cache (A40) and the three accounting columns the composite needs.
--
-- The cache is GLOBAL, not per-user. A query string carries no user data, and
-- per-user rows would multiply storage by the user count for no benefit: two
-- users asking the same question get the same answer from the same tier, so the
-- second is a cache hit rather than a second provider call and a second charge.
-- Nothing here is `+sync`: this is a server-side cost control, not an entity a
-- client edits.
create table search_cache (
  -- `dedupe_key(query, provider, max_results)` in crates/core/src/search, so a
  -- different provider or a different result count is a different row.
  key text primary key,
  -- Which tier produced `results`, for the admin view of what is actually
  -- answering. Kept separately from the key so a row can be inspected without
  -- re-deriving the hash.
  provider text not null,
  -- The ranked `Vec<SearchHit>` as JSON. jsonb rather than a normalized table
  -- because nothing queries inside it: it is read whole or not at all.
  results jsonb not null,
  fetched_at timestamptz not null default now(),
  -- `expires_at` rather than `fetched_at + ttl` so the TTL can change without
  -- rewriting history, and so the purge job has one indexable column.
  expires_at timestamptz not null
);
-- For the purge job, which deletes by expiry and never reads a row.
create index on search_cache (expires_at);

-- A cache hit still writes a `usage_events` row, so the request is accounted for
-- even though it is not charged (A40). Without this the margin report would see
-- fewer web searches than users made and read as growth in something it is not.
alter table usage_events add column cache_hit boolean not null default false;
-- Which tier answered this request: `openalex`, `searxng`, `exa`, ... Nullable
-- because most usage events are not searches.
alter table usage_events add column search_tier text;
-- The engines SearXNG reported as unresponsive for this request. This lives on
-- `usage_events` so silent degradation of the free path shows up in the admin
-- margin report instead of being discovered by a user: an empty result set that
-- is actually "seven of eight engines are down" must not look like "the web had
-- nothing". Nullable, and only set for a SearXNG-answered request.
alter table usage_events add column unresponsive_engines text[];
