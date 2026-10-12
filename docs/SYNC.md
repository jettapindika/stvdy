# Sync

**Status:** normative. Later phases read this to learn the protocol. The code it describes
is `crates/sync` (the wire types and the client engine), `crates/server/src/sync` (the three
routes), `migrations/0006_sync_functions.sql` and `migrations/0027_sync_watermarks.sql`.
Where this document and the code disagree, one of them is a bug and it is fixed in the same
commit.

## Cursor model

`change_log` is the sync clock. Every synced table carries `user_id`,
`updated_at` and `deleted_at`, and two triggers:

- `touch_updated_at` maintains `updated_at` on UPDATE (the server receive time
  that last-write-wins compares).
- `log_change` (or `log_change_by_user` for `user_settings`) appends a
  `change_log` row in the same transaction, so a write and its log entry are
  atomic.

`seq` is a `bigserial`, so it is assigned by `nextval()` at INSERT time while the
row becomes visible only at COMMIT time. Two concurrent transactions for one user
can therefore be assigned seq 10 and 11 and commit in the opposite order. A
client that reads `seq > cursor`, sees 11 and advances to 11 would never receive
10. A permanently skipped change on that device.

**Both trigger functions take `pg_advisory_xact_lock(hashtext(user_id::text))`
before inserting the change_log row.** The lock is held to commit, so for any
committed row of a user every lower seq is already committed, and a reader can
only observe a contiguous prefix. The lock must precede the insert; taking it
after is useless because seq has already been assigned.

That property is the whole reason the cursor is a single integer. It is also why
nothing may compare `seq` across users (the sequence is global) and why nothing may
order by `changed_at` where the cursor is the point: `changed_at` is `now()` at insert
time and is not the ordering the cursor guarantees.

Accepted cost: one user's sync writes serialise against each other, which is free
at this write rate. The same lock key is used for credit spending, so the two
paths serialise rather than deadlock. Scale-out path if it ever matters: add a
`txid` column and only return rows below `pg_snapshot_xmin(pg_current_snapshot())`.

### Lock order is always advisory-then-rows

Because the lock lives in an AFTER trigger, a multi-row transaction would acquire
it *after* its row locks, so two devices editing overlapping rows could deadlock
(`40P01`). `POST /sync/push` therefore takes the same lock as its **first
statement**, before applying any mutation, making the acquisition order uniform
everywhere. The trigger keeps its own lock as the backstop for the dedicated REST
endpoints, which write a single row and cannot invert the order.

The route's statement order is fixed and each position is load-bearing:

1. `select pg_advisory_xact_lock(hashtext($1::text))` — the lock, before any row.
2. `select set_config('app.device_id', $1, true)` — so `origin_device_id` is populated
   without a device column on every table.
3. per mutation: the replay lookup, the decision, the apply under a SAVEPOINT, the
   `mutation_log` insert.

`sync_push.rs::the_push_takes_the_advisory_lock_before_it_touches_a_row` asserts the
first position by observation rather than by reading the file: it holds the advisory lock
in another transaction, drives a push, waits until that push is blocked on the advisory
lock, and then asserts that the push holds **no** `RowExclusiveLock` on `tasks` — a lock
it would hold if it had written a row before taking the advisory lock.

## Pull

`GET /sync/pull?cursor=<seq>&offset=<n>&limit=<n>`

```json
{
  "changes": [{"seq": 42, "entity": "task", "id": "...", "op": "upsert", "data": {}}],
  "next_cursor": 42,
  "has_more": false
}
```

- `limit` defaults to 200 and is clamped at 1000. A `limit` below 1 is a 400, because it is
  a caller bug rather than a request for a smaller page; a `limit` above the cap is the
  caller asking for too much and is clamped, because the cap exists to bound one response
  and not to punish a caller for asking.
- `next_cursor` and `has_more` are the client's whole state. A page carries one extra row
  internally, which is how `has_more` is decided without a second query.
- A tombstone has `op: "delete"` and `data: null`.
- A negative `cursor` or `offset` is a 400 `validation`.
- Every response is scoped by `user_id` **in the statement that reads it** — never by a
  filter applied afterwards. That is a code rule, not a review rule: a `select` followed by
  a filter in Rust is a defect even when it returns the right rows, because the next edit is
  what makes it wrong.

### The initial pass is snapshot-anchored (A164)

`cursor=0` starts the *initial pass*. Its first page is ordered by
`(dependency_rank, seq)` and the server sets `next_cursor` to the account's `max(seq)` at
the moment the pass began — the **anchor**. Every later page of that pass sends
`cursor=<anchor>&offset=<n>`, and every row of the pass satisfies `seq <= anchor`.

The rank is built from `Entity::dependency_rank` and read into the SQL by
`pull::rank_expression()`, so the SQL order and the wire order cannot become two lists:

`workspace(1), source(2), deck(3), flashcard(4), quiz(5), quiz_question(6),
report(7), task(8), goal(9), event(10), study_session(11), message(12),
flashcard_review(13), quiz_attempt(14), user_settings(15), pending_action(16),
cheatsheet(17)`.

The anchor exists because offset paging over a rank-ordered stream can **permanently** skip
a row. A new workspace (rank 1) inserted while the client is deep in rank-3 territory sorts
before the offset the client has already passed, is never read, and the client's cursor —
the anchor from a later page, which is larger — is already past it. The row is lost on that
device for good. With the anchor the pass set is immutable; anything committed during the
pass has `seq > anchor` and arrives on the next incremental pull.

An interrupted pass may restart from `offset=0`: the pass is idempotent, because every
change in it is an upsert of a row the client may already have.

`offset=0` with a non-zero cursor is an incremental request; a non-zero `offset` marks a
page of the initial pass and may travel with the anchor in `cursor`. The wire response gains
no field for any of this (A167): the only addition is the query parameter.

### The incremental walk

After the pass completes, the client sends a bare non-zero `cursor` and the server answers
`seq > cursor` ascending, `next_cursor` = the last seq it sent. On an empty page the cursor
stays where it was, so a client polling a quiet account does not jump its cursor to an
unrelated seq.

An incremental page is complete in itself, so the client commits its cursor per page; a
pass that stops early resumes instead of restarting. The initial pass commits its cursor
only when `has_more` is false, because until then the pass is not finished and the cursor
is the anchor rather than a position.

### Staleness is a watermark, not `min(seq)` (A165)

Compaction writes `change_log_watermark (user_id, min_retained_seq, compacted_at)` per
account. A cursor is stale when

```
cursor > 0 && cursor < min_retained_seq
```

and the answer is **410 `resync_required`** (`AppError::ResyncRequired`, which already
existed — this phase added no error variant and no new body shape, A166).

Comparing against `min(seq)` instead would break new users: `seq` comes from a global
sequence, so a brand-new account's first change can be seq 500, and a device at cursor 0
would be told to resync on its very first request. A cursor of 0 is never stale, so a full
resync is always available.

The client's reaction is not an error the user sees: it wipes, sets the cursor to 0 and
pulls from zero, inside the same pass. A second 410 in one pass is a server fault rather
than a stale cursor, so it is not retried into a loop. **The wipe empties the outbox too**
(A166 and A186 are the same `wipe()` call), which is the price of one wipe: a device that
was offline past the retention window can lose a write it never pushed.

### The payload allowlist (A168)

A change ships the row's current state, restricted to a positive per-entity column list
(`Entity::payload_columns`, in `crates/sync` so both ends read one copy). The list excludes:

- `user_id` — the caller's own, and redundant on every row;
- `sources.file_key` and `sources.preview_key` — internal object-storage keys whose
  client-facing form is a route (`GET /sources/:id/file`), so shipping them would write a
  storage layout into every device's SQLite file;
- `cheatsheets.blocks_key`, `html_key` and `pdf_key` — the same argument.

`created_at`, `updated_at` and `deleted_at` are included: the client needs them for
tombstone handling and for the "Last synced" display. A positive list means a column added
to a table and not to the allowlist is simply not shipped, which is the safe direction.

The loader is `select … from <table> where user_id = $1 and id = any($2)`. That `where` is
the whole defence: a `change_log` row whose `entity_id` belongs to another account loads no
payload at all.

### `data: null` is a delete (A188)

An `upsert` change whose row is gone is emitted as `data: null`, and the client applies
`data: null` as a **delete**, regardless of `op`. A row a device cannot see must never be
created on that device, and this is also what makes the loader's `user_id` scoping safe in
the pathological case: the worst outcome of a foreign `entity_id` is a spurious local
delete, never a cross-tenant read.

**Every change of a live row in one page ships the row, not only the first.** The payload
map is read with `get`, never drained with `remove`. With `remove`, the second and later
changes of a row in a page shipped `data: null`, and because null-on-upsert means "not
visible to you", the client deleted a row the account still has — on any page carrying two
changes for one row, which is the initial pass of any row created and then edited inside the
pass set, and any row that changed twice between two pulls. The convergence suite found it.
The naive repair (never emit null) deletes the signal, so both halves are pinned:
`every_change_of_a_live_row_in_one_page_carries_the_row` and
`a_row_that_cannot_be_loaded_still_ships_a_null_payload`.

## Push and idempotency

`POST /sync/push` takes `device_id` and mutations of
`{mutation_id, entity, op, id, payload, client_ts}`.

- **Idempotent per `mutation_id`** via `mutation_log`: a replay returns the
  stored result and applies nothing. The stored result is returned byte-identical, so a
  client that lost the first response can adopt the answer it already earned.
- `mutation_log.mutation_id` is a **global** primary key, so a collision with a different
  account is representable: that is `Rejected{code: "conflict"}`, never applied and never
  answered with the other account's stored result (A172).
- The body's `device_id` must equal the authenticated device, else **403 `forbidden`**
  (A171). `origin_device_id` exists to attribute a change to the device that caused it, and
  a body free to claim any device id makes that attribution worthless.
- Entitlements and ownership are validated **per mutation**; a rejected mutation
  does not abort the batch. One transaction for the whole batch, one SAVEPOINT per
  mutation, so an apply failure rolls back that mutation alone (A170).
- Results are `applied{seq}`, `rejected{code,message}` or `conflict{winner}` — always in a
  200 batch, one result per mutation in request order. A short answer is a server fault and
  the client retries the whole batch rather than guessing which mutation landed.
- Every apply is `insert … / update … where id = $1 and user_id = $2`, and the upsert's
  conflict clause carries `where <table>.user_id = excluded.user_id`, so another account's
  id affects nothing and is `Rejected{code: "not_found"}` (A173). A separate `select` before
  the write would be a check-then-act a second device can interleave.
- Append-only entities are `on conflict (id) do nothing`, and a duplicate reports the row's
  **existing** seq (read from `change_log`), so a retried review does not read as a new
  change and the client's cursor does not move for a write that changed nothing (A174).
- A mutation whose `entity` or `op` the protocol does not know is a 500 `internal` on the
  pull side, never a skipped row (A169): it can only mean the trigger array and
  `Entity::ALL` have drifted, and a silent skip would be permanent loss on every device.

### Pushable-entity allowlist

Push accepts exactly the spec's offline-write scope — `Entity::is_client_writable`.
Everything else is written through its dedicated endpoint and rejected with
`read_only_entity`.

| Entity | Ops |
|---|---|
| `task`, `event` | upsert, delete |
| `flashcard_review`, `quiz_attempt`, `study_session` | upsert only (append-only) |
| `user_settings` | upsert only (no tombstones) |

Rejected: `workspace`, `source`, `deck`, `flashcard`, `quiz`, `quiz_question`,
`report`, `cheatsheet`, `goal`, `message`, `pending_action`.

Two rejections are security-relevant rather than tidy:

- **`message`** is persisted by `POST /workspaces/:id/chat`. Accepting a push
  would create a second write path that skips the credit spend and lets a client
  forge assistant turns with fabricated citations.
- **`source`** status, `file_key` and `read_only` are server-owned; a client must
  not be able to mark a source `ready` or bypass storage quotas.

A third is a derived-field guard, entity-scoped on purpose: a `flashcard` payload carrying
`ease`, `interval_days` or `due_at` is refused, because those are the output of the
server-side SM-2 replay. It is entity-scoped because a **task** carries a `due_at` the
*client* sets, so a flat rule on the field name would refuse every task push with a due
date.

### Payload shapes (A176)

| Entity | `payload` |
|---|---|
| `task`, `event` | `{"patch": {…}, "base": {…}\|null}` — `base: null` means create |
| every other pushable entity | the full row |

Disjointness cannot be computed from one side: the server keeps no per-field history, so it
needs both the client's changed keys (`patch`) and the row the client edited from (`base`).
The wire type is unchanged — `payload` is `Option<Value>` and its shape is ours;
`Entity::payload_shape()` names which shape an entity uses.

### The merge rule for tasks and events (A177)

The other writer's changed set is `{k : stored[k] != base[k]}`, **excluding `updated_at`**
(the trigger moves it on every write, so comparing it would read as a change on every
mutation and force every edit to look like an overlap). The client's changed set is
`keys(patch)`.

- **Disjoint** — the patch is applied over the stored row and the result is `Applied`. Both
  edits survive. Worked example: device 1 patches `{"title": "B"}` from base
  `{"title": "A", "notes": "n"}` while device 2 has already stored
  `{"title": "A", "notes": "n2"}`. The other writer's changed set is `{notes}`, the client's
  is `{title}`; the result is `{"title": "B", "notes": "n2"}` and both writes are visible on
  both devices.
- **Overlap** — the whole incoming patch wins (record-level last-write-wins on server
  receive time). Worked example: device 1 patches `{"title": "B"}` from the same base while
  device 2 has stored `{"title": "C", "notes": "n"}`. Both changed `title`, so the stored row
  becomes `{"title": "B", "notes": "n"}` — device 2's write to `title` is gone and device 1's
  is the record. A per-field tie-break would need a per-field clock the schema does not have,
  and inventing one would make two devices disagree about which write won.
- **A create** (no base, or `base: null`) has nothing to compare against, so the patch is
  the row.

### `user_settings` is keyed by the user (A179)

The mutation's `id` is ignored on apply; the row is the caller's, and `change_log.entity_id`
is the user id, matching what `log_change_by_user` writes. `Op::Delete` never reaches the
apply, because `accepts_push_op` refuses it — the table has no tombstone column.

## Realtime hint

`GET /sync/stream` (SSE) emits `event: seq` with `data: {"seq": N}` when the user's
change_log advances, so online devices pull immediately. The current `max(seq)` is sent
**immediately on connect**, so a device that reconnects after a gap pulls at once instead of
waiting for the next change. Implemented as a per-connection task polling the caller's own
`max(seq)` every **2 seconds**, with a **15-second** comment heartbeat, which works
correctly with more than one server instance and needs no cross-instance bus (A181). There
is no `LISTEN/NOTIFY` and no cross-instance bus.

`event: seq` is additive to the roadmap §4 event list, which covers the chat stream. The
heartbeat is a comment line (`: keep-alive`), not an event, so a client's parser never has
to read it.

Mobile apps in the background rely on push notifications and pull on foreground:
the SPA's JavaScript cannot run while backgrounded or terminated on either
platform.

## Conflict rules

The table is **data**, in `crates/sync/src/client/conflict.rs` (`Rule` + `rule_for`), and
the server's apply reads it (A175). One copy means the client and the server cannot disagree
about which rule an entity follows. The *decision* stays the server's; the client only
reacts to a `MutationResult`.

| Entity | Rule |
|---|---|
| `flashcard_reviews`, `quiz_attempts`, `study_sessions` | Append-only. Insert; duplicate id ignored. |
| `flashcards` (ease/interval/due) | Server-derived by replaying reviews. Clients treat them as read-only. |
| `tasks`, `events`, `user_settings` | Record-level LWW on server receive time; field-level merge for tasks and events when changed field sets are disjoint. |
| `goals`, `decks`, `reports`, `workspaces`, `cheatsheets` | LWW on server receive time, reached through their dedicated endpoints (not push). |
| Delete vs concurrent edit | Tombstone wins unless the user explicitly restores (`deleted_at = null`, which logs an upsert). |
| `reports.body_md` edited on two devices | Keep latest, store the losing body in `report_revisions`. |
| `sources` status, subscription, credits, planner output, `messages`, `pending_actions` | Server-owned; read-only for clients except via their dedicated endpoints. |

The tombstone rule has exactly one exception and it is the contract's: a patch whose
`deleted_at` is present and `null` restores the row and logs an `upsert` (A178). Without it,
a delete could never be undone from a device. An edit against a tombstoned row is
`conflict{winner}` where the winner is the stored row **including its `deleted_at`**, so the
losing device is told what won rather than silently reverted.

Two writes through push are not one row, and both match the REST routes so the two paths
produce the same `change_log` entries: a **task** tombstone cascades to its live
descendants, and an **event** a connected calendar owns is refused with `read_only_entity`
rather than overwritten. The walk cannot be left to the client — the subtree a device holds
is not the subtree the account has, because another device may have added a subtask since
that one last pulled.

## Offline scope

Native clients can **read** all synced data offline, and **write** offline only
tasks, events, flashcard reviews, quiz attempts, study sessions and settings.
Chat, generation, web search, uploads/ingestion, checkout and calendar connect
require connectivity and queue nothing. They show an offline state instead.

## Local cache and the outbox

The synced entities above (not chunks or embeddings), plus original files
downloaded on demand. Settings offers "Reset local data and resync".

The cache is SQLite owned by the Rust shell (`rusqlite`, `bundled`), never by JS; the
engine that drives it lives in `crates/sync` and is generic over a `LocalStore` trait and a
`Transport` trait, so the same engine runs in the shell over SQLite and in the server's
convergence suite over an in-memory store (A182). `rusqlite` and `reqwest` never enter
`crates/sync`, because the server's test binaries link that crate and a C SQLite in that
graph is a build problem on every platform for no benefit. The web build has no local
database and no outbox (A183): its `write` and `tombstone` refuse and name the REST route,
because a write that resolved while dropping the payload would mark the row saved and the
next pull would overwrite it with the server's older copy, with nothing failing.

**A pass pushes before it pulls.** The outbox goes first, one push per batch of **100**
mutations in enqueue order, so a local write reaches the server before the pull can fetch a
newer copy of the row it touched.

**Retry is bounded exponential backoff**: 500 ms, doubling, capped at 60 s, reset by any
pass that reached the server. The bound keeps a flapping network from becoming a hot loop;
the reset keeps a device that comes back from waiting.

**The status is derived from stored facts only** (A187):

| State | Means |
|---|---|
| `synced` | The outbox is empty **and** the last pass's stored outcome is `synced` **and** `last_synced_at` is known. |
| `syncing` | A pass is in flight, or work is outstanding (a queued write, or no pass has completed yet). |
| `offline` | The last pass failed transiently — the network, a rate limit, a server fault. Waiting is the remedy. |
| `needs_attention` | The last pass left a mutation the server refused, shown with the server's own `code`. |
| `revoked` | This device's session is gone and its local state was wiped (A186). |

No status ever reads "Synced" while the outbox is non-empty. `syncing` is derived, never
stored: only terminal states are written, so a process killed mid-pass does not come back
claiming to be mid-pass. "Last synced …" renders from the stored `devices.last_sync_at`
timestamp — written by pull and by push, in the same transaction as the work (A180) — and
never from a locally invented time.

### A revoked device is wiped (A186)

Any authenticated sync route answers **401** for a revoked device (revoking a device kills
its sessions in the same transaction). The engine treats a 401 on its own device as
`Revoked`, emits `stvdy://sync-revoked`, wipes the cache **including the outbox**, deletes
the keychain refresh token, and the SPA clears its session. The outbox matters: a pending
offline write from a revoked device must not be replayed by the next account that signs in
on that machine.

## Compaction and resync

The `compact_change_log` job trims `change_log` by age, with **two windows** (A184):
`upsert` rows older than `CHANGE_LOG_RETENTION_DAYS` (default **90**) are deleted, and
`delete` rows are kept for `CHANGE_LOG_TOMBSTONE_RETENTION_DAYS` (default **180**, at least
the retention window). A tombstone and an upsert are not the same kind of fact: an old
upsert is redundant, because the row it describes is already on every device that cares,
while a device that has been offline for three months must still be told that a row was
*removed* rather than silently keep its stale copy.

The job writes the per-user watermark in the same transaction as the delete, and runs daily
under `dedupe_key = "compact_change_log:<UTC date>"`, which is what makes it safe to enqueue
from every scheduler pass on every instance. An account with nothing old enough writes **no**
watermark: writing `min(seq)` unconditionally would tell a device holding an older cursor to
resync for no reason.

A client whose cursor is below the watermark gets `410 resync_required` and restarts from 0,
as above. Account deletion cascades its `change_log` rows away, and the trigger deliberately
does **not** log deletes whose owner no longer exists. Those tombstones could never be
pulled.

## Rate limiting

The three sync routes are rate-limited **per account at 600 requests per minute**
(`ratelimit::sync_layer`, sharing the spend cap's token-bucket helper). The limit is far
above a real device's rate — a pull per change plus a reconnect — so a legitimate burst is
never what trips it and a stream reconnect is not abuse. The bucket is keyed on the
authenticated account, so an unauthenticated caller cannot fill someone else's bucket.

## The isolation rule

Every read on this path is scoped by `user_id` **in the statement that reads it**, never by
a filter applied afterwards. Three tests make that structural rather than claimed:
`sync_pull.rs::a_pull_never_contains_another_accounts_seq`,
`sync_pull.rs::the_payload_loader_cannot_be_pointed_at_another_users_row`, and
`sync_convergence.rs::two_accounts_never_cross`, which asserts on the **seq values** each
store ever applied rather than on the absence of a 200, and first asserts both stores
applied something so the assertion cannot pass by iterating nothing.

## Latency

One `tracing::info!` per request: `sync pull` logs `user_id`, `device_id`, `rows`, `cursor`,
`has_more` and `latency_ms`; `sync push` logs `user_id`, `device_id`, `mutations`, `applied`
and `latency_ms`. The roadmap names sync pull/push as the paths to log latency for, and a
slow or an empty pull is exactly what these lines make visible.

## Calendar-sourced events (Phase 10)

The four columns the calendar sync owns — `source`, `external_id`, `etag`, `connection_id`
(A198) — change what the row's conflict rule is, and the split is by **column** rather than by
row:

| Row | Rule |
|---|---|
| `events` with `source = 'app'` | Unchanged: record-level LWW on server receive time, with field-level merge when the changed field sets are disjoint. |
| `events` with `source != 'app'` | **Server-owned for the calendar columns**, and read-only to a client on **both** write paths. `PATCH`/`DELETE` answer 403 `forbidden` naming the calendar; push answers `read_only_entity`. The next incremental sync would revert an edit that landed, so an accepted edit is a lie about what the row says. |
| `events.title` / `starts_at` / `ends_at` on an **app-owned** row | Still LWW + field merge, exactly as above. An external row's are not: the sync writes them from Google or ICS. |
| `calendar_connections` | **Not `+sync`.** The connection row holds an encrypted refresh token and a feed URL, and the feed itself is a capability URL (A242); a client that could pull or push one would be holding a credential the server is supposed to own. |
| `users.ics_feed_token` | **Not `+sync`** for the same reason: it is a bearer secret, and `users` is not a synced entity. |

**The read-only rule lives in the routes and the push validator, never in a trigger** (A240).
The ICS importer and the Google sync write the four columns through plain SQL, so a trigger
that refused the write would refuse the sync's own write — and the only way to keep the two
apart would be a session variable, a second mechanism for a distinction the two paths already
make structurally. `crates/server/tests/calendar.rs` asserts the non-obvious half: a row the
sync wrote is refused by `PATCH`, and the sync can still write it.

**No new `+sync` table, and `0017_sync_triggers.sql` is unchanged.** The ICS and Google rows
land in `events`, which already has its triggers, so this phase attaches none and
`crates/server/tests/schema.rs` needs no new row — the precedent `0021`–`0023`, `0028` and
`0029` each state in their own migration header.

**Deleting a connection tombstones its events in the same transaction.** `events.connection_id`
is `references calendar_connections on delete set null`, so deleting the connection alone
would leave orphan `source = 'ics'` rows that no sync will ever touch again and that a client
cannot edit or delete either — a timetable the student cannot remove. A tombstone rather than
a hard delete, because a row removed outright leaves every device showing the event forever
with no change-log entry to correct it.
