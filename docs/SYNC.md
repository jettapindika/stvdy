# Sync

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

## Pull

`GET /sync/pull?cursor=<seq>&limit=<n>`

```json
{
  "changes": [{"seq": 42, "entity": "task", "id": "...", "op": "upsert", "data": {}}],
  "next_cursor": 42,
  "has_more": false
}
```

- Ordered by `seq asc`; `limit` default 200, max 1000.
- A tombstone has `op: "delete"` and `data: null`.
- A cursor older than retained history returns `410 resync_required`; the client
  then restarts from cursor 0.
- Initial sync (`cursor=0`) is ordered by dependency rank, then seq:
  `workspace(1), source(2), deck(3), flashcard(4), quiz(5), quiz_question(6),
  report(7), task(8), goal(9), event(10), study_session(11), message(12),
  flashcard_review(13), quiz_attempt(14), user_settings(15), pending_action(16),
  cheatsheet(17)`.

## Push and idempotency

`POST /sync/push` takes `device_id` and mutations of
`{mutation_id, entity, op, id, payload, client_ts}`.

- **Idempotent per `mutation_id`** via `mutation_log`: a replay returns the
  stored result and applies nothing.
- Entitlements and ownership are validated **per mutation**; a rejected mutation
  does not abort the batch.
- Results are `applied{seq}`, `rejected{code,message}` or `conflict{winner}`.

### Pushable-entity allowlist

Push accepts exactly the spec's offline-write scope. Everything else is written
through its dedicated endpoint and rejected with `read_only_entity`.

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

## Realtime hint

`GET /sync/stream` (SSE) emits `{seq}` when the user's change_log advances, so
online devices pull immediately. Implemented as a per-connection task polling
`max(seq)` every ~2s, which works correctly with more than one server instance
and needs no cross-instance bus.

Mobile apps in the background rely on push notifications and pull on foreground:
the SPA's JavaScript cannot run while backgrounded or terminated on either
platform.

## Conflict rules

| Entity | Rule |
|---|---|
| `flashcard_reviews`, `quiz_attempts`, `study_sessions` | Append-only. Insert; duplicate id ignored. |
| `flashcards` (ease/interval/due) | Server-derived by replaying reviews. Clients treat them as read-only. |
| `tasks`, `events`, `user_settings` | Record-level LWW on server receive time; field-level merge for tasks and events when changed field sets are disjoint. |
| `goals`, `decks`, `reports`, `workspaces`, `cheatsheets` | LWW on server receive time, reached through their dedicated endpoints (not push). |
| Delete vs concurrent edit | Tombstone wins unless the user explicitly restores (`deleted_at = null`, which logs an upsert). |
| `reports.body_md` edited on two devices | Keep latest, store the losing body in `report_revisions`. |
| `sources` status, subscription, credits, planner output, `messages`, `pending_actions` | Server-owned; read-only for clients except via their dedicated endpoints. |

## Offline scope

Native clients can **read** all synced data offline, and **write** offline only
tasks, events, flashcard reviews, quiz attempts, study sessions and settings.
Chat, generation, web search, uploads/ingestion, checkout and calendar connect
require connectivity and queue nothing. They show an offline state instead.

## Local cache contents

The synced entities above (not chunks or embeddings), plus original files
downloaded on demand. Settings offers "Reset local data and resync".

## Compaction and resync

A job trims `change_log` older than the retention window (default 90 days) but
keeps tombstones at least as long. A client whose cursor is older than the
retained window gets `410 resync_required` and restarts from 0. Account deletion
cascades its `change_log` rows away, and the trigger deliberately does **not**
log deletes whose owner no longer exists. Those tombstones could never be pulled.
