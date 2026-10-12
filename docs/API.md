# API

Base URL is the server origin. Every route below requires
`Authorization: Bearer <access_token>` unless marked public. Bodies and responses
are JSON unless noted.

## Conventions

- **Errors** are always `{"code", "message", "min_plan", "limit", "used"}`, with
  all five keys present and the non-applicable ones `null`, so the client's
  error mapper never has to guess. `code` is machine-readable and stable;
  `message` is for logs, not for showing to users. An entitlement error fills
  `min_plan`, `limit` and `used`, so the UI can render "3 of 3 devices" rather
  than only "upgrade".
- **Status codes**: 401 unauthorized/expired token, 402 insufficient credits,
  403 plan required, feature locked, or `email_not_verified`, 404 not found,
  409 conflict, 410 `resync_required` or `link_expired`, 422 limit reached
  (`limit_reached`), 429 rate limited, 502 provider error.
- **Expired versus tampered links**: a signed URL past its expiry is 410
  `link_expired`, because the remedy is to fetch a fresh one; a signature that
  does not match is 403. They are separated so a client can tell a retryable
  condition from an attack.
- **Times** are ISO 8601 UTC. Local rendering uses the user's IANA timezone from
  `GET /settings` (default `Asia/Jakarta`).
- **IDs** are UUIDv7. Clients may supply ids on create; the server accepts them.
- **Pagination** is cursor-based for sync and offset-based elsewhere.

### SSE

Chat and sync-stream routes answer `text/event-stream`. Named events:

```
event: token      data: {"t":"..."}
event: citations  data: {"citations":[{"type":"chunk","chunk_id":"...","source_id":"...","page":12}]}
event: action     data: {"pending_action_id":"...","tool":"create_task","args":{...}}
event: usage      data: {"credits_spent":1,"credits_remaining":412}
event: done       data: {"message_id":"...","used_web":false}
event: error      data: {"code":"insufficient_credits","message":"...","min_plan":null}
event: seq        data: {"seq":42}
```

`seq` is the sync stream's event and belongs to `GET /sync/stream` alone (A181):
it is the caller's own `max(seq)`, sent on connect and on every advance, and its
only job is to tell an online device to pull now. The chat stream never emits it.
A comment heartbeat (`: keep-alive`) goes out every 15 seconds on both.

Consume with `fetch` + a stream reader, not `EventSource`: chat needs POST and an
`Authorization` header, neither of which `EventSource` supports.

## Phase 0 (implemented)

### `GET /health`

Public. Liveness plus a cheap readiness probe.

```json
{ "status": "ok", "version": "0.1.0", "env": "dev", "plans": 3 }
```

Returns 500 if the database is unreachable.

## Auth (Phase 1)

The credential-taking routes (`signup`, `login`, `refresh`, `logout`,
`verify-email`, `google/exchange`) are public and rate limited at 30 requests per
minute per IP; the routes marked Auth below require a bearer token and are not in
that bucket.
An access token is an HS256 JWT carrying `sub`, `did`, `sid`, `iat`, `exp`, `jti`
and deliberately no plan claim (A22): editing it grants nothing. A refresh token
is 32 random bytes, base64url, stored only as a SHA-256 hex in `auth_sessions`
and rotated on every use.

`UserView`:

```json
{ "id": "uuid", "email": "a@b.com", "email_verified": false,
  "email_verified_at": null, "plan_id": "free", "timezone": "Asia/Jakarta",
  "student_verified": false, "created_at": "2026-10-11T00:00:00Z" }
```

`AuthResponse` (signup, login, Google exchange):

```json
{ "user": { "...UserView" }, "access_token": "jwt", "refresh_token": "opaque",
  "expires_at": "2026-10-11T00:15:00Z", "device_id": "uuid" }
```

### `POST /auth/signup`

Body: `{ "email", "password", "device": { "name", "platform", "app_version?" } }`.
`platform` is one of `web`, `macos`, `windows`, `ios`, `android`. Creates the
user, the device and a session, and mails a verification link.

201 `AuthResponse`. 400 `validation` for a malformed email, a short password or
an unknown platform. 409 `conflict` when the email already exists. 429
`rate_limited`.

### `POST /auth/login`

Body: `{ "email", "password", "device": { "name", "platform", "app_version?" } }`.
200 `AuthResponse`. An unknown email, a wrong password and a Google-only account
all return the same 401 `unauthorized` after the same argon2 work, so the
response time does not reveal whether an address is registered. 422
`limit_reached` with `min_plan` when `devices_max` is reached; a revoked device
does not count against the limit.

### `POST /auth/refresh`

Body: `{ "refresh_token" }`. 200
`{ "access_token", "refresh_token", "expires_at" }`. Rotates: the presented token
is revoked and a new one is issued in the same `family_id`. Presenting an
already-rotated token is treated as theft and revokes the whole family (401
`unauthorized`); an unknown or expired token is 401 as well.

### `POST /auth/logout`

Body: `{ "refresh_token" }`. 204. Revokes the token's session family. Idempotent,
so logging out twice is not an error.

### `POST /auth/logout-all`

Auth. No body. 204. Revokes every live session of the caller.

### `POST /auth/verify-email`

Body: `{ "token" }`. 200 `{ "verified": true }`. Single use, enforced by the
`used_at is null` predicate in the UPDATE, so two concurrent requests cannot both
succeed. 400 `validation` when the link is invalid or expired.

### `POST /auth/resend-verification`

Auth. No body. 204. A no-op (204) when the address is already verified; a resend
invalidates any earlier unused token, so only one live link exists per account.

### `GET /auth/me`

Auth. 200 `UserView`.

## Auth: Google (Phase 1)

Authorization Code + PKCE, server-side (A48). Google login is optional per
deployment: with no `GOOGLE_LOGIN_CLIENT_ID` configured, `start` answers 404
rather than redirecting to a half-configured client.

### `GET /auth/google/start`

Public. Query: `device_name?`, `device_platform?`, `app_version?`. 302 to
Google's authorization endpoint with `scope=openid email profile`,
`code_challenge_method=S256` and `access_type=online` (no refresh token is
needed for login). The `state` parameter is AES-GCM encrypted, not just signed:
it carries the PKCE verifier and the nonce, and a readable `state` would hand the
verifier to the browser, which is the party PKCE exists to distrust. 404 when
Google login is not configured.

### `GET /auth/google/callback`

Public. Query: `code`, `state`, `error`. Decrypts `state`, exchanges the code,
verifies the `id_token` against Google's JWKS (issuer, audience, expiry, nonce
and `email_verified`), then resolves or creates the account. Responds 302 to
`stvdy://auth/callback?ticket=<ticket>`. 400 `validation` when the user cancelled
(`error` present), or when `state` or `code` is missing or tampered with.

The redirect carries a single-use ticket, never tokens. A deep link lands in
browser history and the OS URL handler, so a credential placed there could not be
revoked; the ticket is stored only as a hash, expires in five minutes and is
consumed once.

### `POST /auth/google/exchange`

Public, rate limited. Body:
`{ "ticket", "device": { "name", "platform", "app_version?" } }`. Consumes the
ticket and returns 200 `AuthResponse`, the same pair `POST /auth/login` returns.
The device is created here, not in the callback, so a browser round trip cannot
consume a device slot on its own. 401 `unauthorized` for an unknown, used or
expired ticket.

### `POST /auth/google/set-password`

Auth. Body: `{ "password" }`. 204. Gives a Google-only account a password. 403
`email_not_verified` until the address is confirmed, or the reset path becomes a
takeover vector.

## Devices (Phase 1)

All routes require auth. A device is the unit of "sign out that old laptop":
revoking one sets `revoked_at` and kills its sessions in the same transaction.
Every lookup is scoped by `user_id` and answers 404 for another user's row (never
403: whether an id exists is not information this endpoint leaks). `GET /devices`
returns live devices only.

`DeviceView`:

```json
{ "id": "uuid", "name": "MacBook", "platform": "macos", "app_version": null,
  "last_seen_at": "2026-10-11T00:00:00Z", "created_at": "2026-10-11T00:00:00Z",
  "revoked_at": null, "current": true }
```

`current` is true for the device making the request, so the client can mark "this
device" without guessing from a name it may have renamed.

- `GET /devices` → 200 `{ "devices": [ "...DeviceView" ] }`.
- `PATCH /devices/:id` body `{ "name" }` → 200 `{ "device": "...DeviceView" }`.
  400 when the name is empty or over 120 characters; 404 for another user's or a
  revoked device.
- `DELETE /devices/:id` → 204. Revokes the device and every session it holds, so
  its next request is 401. 404 when it is not the caller's live device.
- `PUT /devices/:id/push-token` body `{ "provider", "token" }` → 204. `provider`
  is `apns` or `fcm`. Upsert keyed by `device_id`, so the OS rotating a token
  does not double every notification. 400 for an unknown provider or an empty
  token; 404 for a device that is not the caller's live device.

## Sync (Phase 8)

Auth, and rate-limited per account at **600 requests/minute** (A185). The three
routes are the client engine's whole wire surface; the normative description of
the protocol is `docs/SYNC.md`, and this section is its request/response
contract.

### `GET /sync/pull`

Query parameters, all optional:

| Parameter | Default | Notes |
|---|---|---|
| `cursor` | `0` | `0` starts the **initial pass**; any other value is a position on the `seq` clock. A negative value is a 400 `validation`. |
| `offset` | `0` | Marks a page of the initial pass. Legal with any cursor: the first page sends `cursor=0&offset=0`, later pages send `cursor=<anchor>&offset=<n>`. |
| `limit` | `200` | Clamped to a maximum of `1000`. Below `1` is a 400 `validation` — a caller bug rather than a request for a smaller page. |

```json
{
  "changes": [
    { "seq": 42, "entity": "task", "id": "uuid", "op": "upsert", "data": { "…": "…" } }
  ],
  "next_cursor": 42,
  "has_more": false
}
```

- `changes` is ordered by `seq` ascending on the incremental walk and by
  `(dependency_rank, seq)` on the initial pass, so a parent lands before its child.
- `next_cursor` is the anchor of the pass while the initial pass is unfinished, and
  the last `seq` sent once it is. On an empty incremental page it is the cursor the
  request carried, so polling a quiet account does not jump the cursor.
- `has_more` is decided from one extra row read, so a full page is not a last page.
- `data` is the row restricted to the per-entity column allowlist (A168). It never
  contains `user_id`, `sources.file_key`, `sources.preview_key` or the cheatsheet
  object keys.
- A tombstone is `{"op": "delete", "data": null}`. An `upsert` whose row this
  account no longer has is also `data: null`, which the client applies as a delete
  (A188) — so every change of a live row in a page carries the row, not only the
  first.
- **410 `resync_required`** when `cursor > 0 && cursor < min_retained_seq` (the
  per-user compaction watermark, A165). The remedy is a full resync: pull with
  `cursor=0`. A cursor of `0` is never stale.

### `POST /sync/push`

```json
{ "device_id": "uuid",
  "mutations": [
    { "mutation_id": "uuid", "entity": "task", "op": "upsert", "id": "uuid",
      "payload": { "patch": { "…": "…" }, "base": { "…": "…" } },
      "client_ts": "2026-10-11T00:00:00Z" }
  ] }
```

One transaction for the whole batch, one SAVEPOINT per mutation, so a rejected
mutation does not abort the batch. `mutation_id` is the idempotency key: a replay
returns the stored result byte-identical and applies nothing.

`payload`'s shape depends on the entity (A176): `task` and `event` carry
`{"patch": {…}, "base": {…}|null}` with `base: null` meaning create, and every
other pushable entity carries the full row.

The response is always 200 with one result per mutation, in request order:

```json
{ "results": [ { "status": "applied", "seq": 42 } ] }
```

| Result | Body | Means |
|---|---|---|
| Applied | `{"status":"applied","seq":N}` | Written; `N` is the change's `seq`. |
| Rejected | `{"status":"rejected","code":"…","message":"…"}` | Not written. `code` is the machine-readable one from `docs/SYNC.md`'s table. |
| Conflict | `{"status":"conflict","winner":{…}}` | A tombstone won over this edit (A178). `winner` is the stored row **including its `deleted_at`**. |

Refusals and their codes:

| Code | When |
|---|---|
| `read_only_entity` | The entity is outside the six-entity offline scope, the op is not accepted for it (a delete of an append-only row, any delete of `user_settings`), or the payload carries a server-derived field (`ease`, `interval_days`, `due_at` on a `flashcard`). |
| `not_found` | The id is another account's, or the row is already gone. Deliberately one answer for both: a client that could tell them apart could probe for another account's ids (A173). |
| `conflict` | The `mutation_id` is recorded for a **different** account (A172). Never that account's stored result. |
| `validation` | The entity's own field rules refused the payload — the same functions the REST routes call. |
| `limit_reached` / `plan_required` / `feature_locked` | The entitlement engine's refusal for this entity's features. |
| `internal` | The apply failed for a server-side reason. That mutation alone was rolled back to its savepoint. |

Whole-request failures: **403 `forbidden`** when the body's `device_id` is not the
authenticated device (A171); **400 `validation`** for a malformed body; **429
`rate_limited`** above 600/minute; **401** for a revoked device, whose sessions are
killed with the device.

### `GET /sync/stream`

`text/event-stream`. Emits `event: seq` with `data: {"seq": N}` whenever the
caller's own `max(seq)` advances, and sends the current value **immediately on
connect**, so a device that reconnects after a gap pulls at once. A per-connection
poll every **2 seconds** with a **15-second** comment heartbeat (A181): a poll
stays correct behind more than one server instance and needs no cross-instance bus.
`event: seq` is additive to the chat event list above.

## Workspaces (Phase 1)

Auth. Deletes are tombstones (`deleted_at`), and a workspace delete tombstones
its sources in the same transaction, so the source list cannot outlive its
workspace. Every read is scoped by `user_id` and answers 404 for another user's
row.

`WorkspaceView`:

```json
{ "id": "uuid", "name": "Cell biology", "created_at": "2026-10-11T00:00:00Z",
  "updated_at": "2026-10-11T00:00:00Z", "source_count": 3 }
```

`source_count` counts live sources only.

- `GET /workspaces` → 200 `{ "workspaces": [ "...WorkspaceView" ] }`, most
  recently updated first.
- `POST /workspaces` body `{ "name", "id?" }` → 201
  `{ "workspace": "...WorkspaceView" }`. The client may supply `id` (A8), which
  is what lets a row be created offline and synced later. 400 for an empty name;
  409 when that id already exists; 422 `limit_reached` with `min_plan`, `limit`
  and `used` when the plan's `workspaces` limit is reached.
- `GET /workspaces/:id` → 200 `{ "workspace": "...WorkspaceView" }`. 404 for
  another user's or a deleted row.
- `PATCH /workspaces/:id` body `{ "name" }` → 200
  `{ "workspace": "...WorkspaceView" }`. 400 for an empty name, 404 otherwise.
- `DELETE /workspaces/:id` → 204. Tombstones the workspace and its sources. 404.

## Sources (Phase 1)

Auth unless noted. Upload is validated against the plan before the bytes are
stored, so an over-quota file never occupies the bucket.

`SourceView`:

```json
{ "id": "uuid", "workspace_id": "uuid", "kind": "pdf", "title": "chapter1",
  "url": null, "file_size": 1048576, "mime": "application/pdf",
  "status": "ready", "error": null, "created_at": "2026-10-11T00:00:00Z" }
```

- `POST /workspaces/:id/sources` (multipart) → 201 `{ "source": "...SourceView" }`.
  Fields: `kind` in `pdf`, `pptx`, `docx`, `url`, `text` or `web` (Phase 7 adds
  the last four); then the kind's own payload — `file` for `pdf`/`pptx`/`docx`,
  `text` for `text`, `url` for `url`/`web`; optional `title` (defaults to the
  filename, the link's host, the page's own title, or the first non-empty line of
  pasted text). The source is stored `pending` and an `ingest_source` job is
  enqueued. 400 `validation` for a missing or unsupported `kind`, a missing
  payload, a filename whose extension does not match its `kind` (`.pdf`, `.pptx`,
  `.docx` — the extension is the check, because the two OOXML kinds share a
  container and a browser's content type is a guess), a link the SSRF guard
  refuses, a page with no readable text, or empty text. 404 for another user's
  workspace. 422 `limit_reached` for `sources_total`, `max_file_mb` or
  `storage_mb`.
  - **`url`** — a link the user typed. The scheme and host are checked at the
    request (so a bad link is an immediate 400); the page itself is fetched by
    the ingest job, so the request does not block on someone else's server and an
    SSRF refusal lands on `sources.error`. A `url` source stores no bytes: it is
    bounded by `sources_total` and charges no storage (A162/A163).
  - **`web`** — "save to workspace" from a search hit. The page is fetched
    **during the request**, because saving means preserving what was saved; the
    fetched text is the source's object, so it does charge storage (A162/A163).
- `GET /sources/:id/preview` → 200 `{ "url", "expires_at" }`, a short-lived URL
  for the PDF rendition of a `pptx` or `docx` source, so the client opens one
  media type for every kind. **404** when the source has no rendition — no
  `SOFFICE_BIN` converter is configured (A158), or the source has no original
  file (a `url` source) — rather than serving an empty document that would look
  like a scan. A read of the caller's own source, so it is never
  entitlement-gated (A149).
- `GET /workspaces/:id/sources` → 200 `{ "sources": [ "...SourceView" ] }`.
- `GET /sources/:id/status` → 200
  `{ "id", "status", "chunk_count", "error", "updated_at" }`. `status` is
  `pending`, `processing`, `ready` or `failed`; `error` is the readable failure
  the client shows verbatim.
- `GET /sources/:id/file` → 200 `{ "url", "expires_at" }`, a short-lived URL for
  the original. On the local backend the URL points at `/objects/...` below; on
  S3 it is a presigned bucket URL. The client contract is the same on both.
- `DELETE /sources/:id` → 204. Tombstones the row, drops its chunks and deletes
  the object (a storage failure is logged, not fatal).
- `GET /chunks/:id` → 200 `{ "id", "source_id", "page", "idx", "content" }`.
  `page` is 1-based and null for pasted text. 404 for a chunk of another user's
  source or of a tombstoned source.
- `GET /objects/:id?exp=&sig=` (public, no bearer token) → the original bytes.
  This is the local storage backend's signed download: the HMAC signature is the
  authorization, which is what makes it equivalent to an S3 presigned URL from
  the client's point of view. An expired signature is **410 `link_expired`** and a
  mismatched one is **403**, because the remedies differ: an expired link needs a
  fresh URL, a mismatch means the link was tampered with and must not be retried.
  404 on the S3 backend, where this route is not part of the contract.

## Chat (Phase 2, implemented)

### `POST /workspaces/:id/chat`

Auth (and the email must be verified: chat is one of the four actions A43 keeps
locked until then, so an unverified account gets 403 `email_not_verified`).
Answers `text/event-stream`.

Body: `{"message": string, "source_ids"?: [uuid], "use_web"?: bool,
"sources_only"?: bool, "study_mode"?: bool}`. `source_ids` narrows retrieval to the
selected sources; an absent or empty list searches the whole workspace.

`sources_only` is the per-chat "Answer only from my sources" toggle. It defaults to
`true` when omitted, because that is the UI default: a request that drops the field
must behave as grounded ON rather than silently answering from general knowledge.
With it on, the prompt forbids an answer from general knowledge and tells the model
to offer web search or the toggle itself instead; with it off, a general-knowledge
answer is allowed but must be labelled as not coming from the user's sources and
carry no citations.

It is independent of `use_web`: web results are still retrieved, still arrive as
`<web_results>`, and are still citable as `[W#]` when the toggle is on. The toggle
governs general knowledge, not web. A turn composed with it on logs
`prompt_version = "base.v2"` (A38) on its `usage_events` row.

`use_web` runs a web search and puts the results in the prompt as a separate
`<web_results>` block, labelled `[W1]`, `[W2]`. A tier that fails is recorded and
the walk continues: a query no tier answered is not an error, and the turn answers
from the workspace's own sources. A cache hit is not charged; a search that called a
provider costs `web_search` credits on top of the message.

`used_web` on the stored turn means the turn **used** web context, not that the
toggle was on: a toggle-on turn where every tier returned nothing stores false.

A web citation is `{"type":"web","url":"...","title":"...","doi":"..."|null}`.
`doi` is present only when the tier provided one (the academic tier does), and it
is the stable identifier: the same paper is often reachable at two hosts.

Order of operations, which is the contract rather than an implementation detail:
verified email, owned workspace, `use_web` refusal, provider configured, daily
message cap, retrieve, compose, credit spend, persist the user message, stream. Every check that can fail therefore fails
*before* a credit is spent and before anything is written, so a refused request
leaves no trace and can be retried safely. That is why `insufficient_credits`
arrives as a normal HTTP 402 rather than as a mid-stream event.

Events, in this order:

```
event: token      data: {"t":"..."}                    # repeated; concatenate in order
event: citations  data: {"citations":[...]}            # always sent, even when empty
event: usage      data: {"credits_spent":1,"credits_remaining":412}
event: done       data: {"message_id":"...","used_web":false}
event: error      data: {"code":"...","message":"...","min_plan":null,"limit":null,"used":null}
```

`citations` is sent even when the array is empty, so a client can tell "no
citations" from "the stream died". Every exit path emits exactly one terminator
(`done` or `error`): an answer that came back empty yields `error`, because there
is no message id to report and the turn produced nothing. `error` can also arrive **mid-stream**, after
tokens, if the assistant message fails to persist: a client must handle an `error`
at any position. A `: heartbeat` comment line is sent every 15 seconds to keep a
proxy from closing an idle stream, and a client's parser must ignore comment
lines rather than read them as events.

Citation labels are numeric (`[1]`, `[2]`) and resolve against the context the
server sent. A label the model invents is dropped from the **streamed** text as
well as the stored copy (A17): the server holds back a partial marker and emits a
complete one only if the label is known, so a rejected marker never reaches the
screen. The client therefore never sees a citation it cannot open.

The assistant message is persisted **after** the stream completes, with the
resolved citations. A client that disconnects mid-answer therefore leaves the
user's turn and no half-answer.

### `GET /workspaces/:id/messages`

Auth. 200 `{"messages":[{"id","workspace_id","role","content","citations",
"used_web","created_at"}]}`, oldest first, where `role` is `user` or `assistant`
and `citations` is the same array shape as the event. This is what makes a thread
visible on the account's other devices. Another account's workspace is a 404.

## Flashcards (Phase 4)

Generation is asynchronous: `POST /workspaces/:id/decks` returns 201 immediately
with a deck in status `generating`, and the client polls `GET /decks/:id` until
the status settles to `ready` or `failed`. The card's schedule is the server's,
derived from the review rows (see `docs/ARCHITECTURE.md`); the client never
authors `ease`, `interval_days` or `due_at`.

`Flashcard = {"id", "deck_id", "front", "back", "chunk_id", "ease",
"interval_days", "due_at"}`. `chunk_id` is null when the model cited a label that
was not in the prompt's context: the card is kept and only its link is lost (A17).

### `POST /workspaces/:id/decks`

Auth, and the email must be verified: generation is one of the four actions A43
keeps locked until then, so an unverified account gets 403 `email_not_verified`.

Body: `{"title": string, "source_ids": [uuid], "count": number}`. `count` is how
many cards to ask for; the generator may return fewer and is told never to pad.

201 `{"deck": {"id", "workspace_id", "title", "source_ids", "status":
"generating", "error": null, "requested_count", "card_count": 0, "created_at"}}`.
The body is the same `Deck` shape every other route serialises, `created_at`
included.

- 400 `validation` when the title is empty or over 120 characters, `count` is
  outside `1..=50`, `source_ids` is empty or longer than 20, or a source is not
  this workspace's, not this user's, or not `ready`.
- 402 `insufficient_credits` when the cost exceeds the balance. The cost is
  `ceil(count / 10) × flashcard_batch_10`, i.e. **4 credits per 10 requested
  cards** (21 cards is three batches). It is quoted from the *requested* count; a
  request larger than the generator will produce is refused rather than truncated,
  so the quote can never overcharge (A84).
- 422 `limit_reached` when the request would cross the plan's
  `flashcards_per_month`, counted over the user's **local calendar month** (A85).
  The count includes the cards that decks still generating have requested, so a
  burst of concurrent requests cannot each pass on the last slot; the check, the
  charge and the deck row are one transaction for the same reason.
  The body carries `limit` and `used`.
- 403 `email_not_verified`.

Every check that can fail runs before the credit is spent, so a refused request
costs nothing.

### `GET /workspaces/:id/decks`

Auth. 200 `{"decks": [Deck], "cost": {"per_batch", "batch_size",
"max_cards"}}`, newest first, tombstones excluded. `cost` is the credit price
descriptor the studio panel renders — `per_batch` is the plan matrix's
`flashcard_batch_10`, `batch_size` the number of cards one batch covers, and
`max_cards` the request cap `POST /workspaces/:id/decks` enforces — so the client
never quotes a hardcoded price or limit. A client that ignores `cost` still reads
`decks` unchanged. **This route is a deliberate addition beyond the spec's route
list** (A81): without it a deck is unreachable after a page reload, which makes
the phase's own exit gate untestable by hand.

### `GET /decks/:id`

Auth. 200 `{"deck": Deck, "cards": [Flashcard]}`, cards ordered `(created_at,
id)`. Another account's deck is 404. `Deck.status` is `generating | ready |
failed`; a `failed` deck carries a readable `error` and its credits are refunded
(A87).

### `GET /decks/:id/due?limit=N`

Auth. 200 `{"cards": [Flashcard], "now": <RFC3339>}`. `limit` defaults to 20 and
is clamped to `1..=100`; cards are ordered `(due_at, id)` so a refetch cannot
reshuffle the queue under the student's finger. Another account's deck is 404.

### `POST /flashcards/:id/review`

Auth. Body `{"id"?: uuid, "rating": "again"|"hard"|"good"|"easy",
"reviewed_at"?: RFC3339}`. `id` is client-generated so a retry after a lost
response cannot apply twice; `reviewed_at` defaults to the server's clock.

200 `{"applied": bool, "card": {"id", "ease", "interval_days", "due_at"}}`.
`applied: false` means the review id had already been applied, so the call is
idempotent. The card's state is the replay of its whole review history, so two
devices converge on one schedule rather than on whichever wrote last (A82).

- 400 `validation` for an unknown rating or a `reviewed_at` more than 24 hours in
  the future: a device with a wrong clock would otherwise pin a card out of the
  queue for months.
- 404 for another account's card.

## Quiz (Phase 5)

Generation is asynchronous, exactly as a deck's is: `POST /workspaces/:id/quizzes`
returns 201 immediately with a quiz in status `generating`, and the client polls
`GET /quizzes/:id` until it settles to `ready` or `failed`.

**The score is derived, never stored.** `GET /quizzes/:id/results` computes it from
the attempts on every read, which is what makes attempts from several devices
conflict-free: there is no stored score for two devices to disagree about, and a
replayed attempt cannot double-count because it is the same row.

`Quiz = {"id", "workspace_id", "title", "difficulty", "question_types",
"source_ids", "status", "error", "requested_count", "question_count",
"created_at"}`.
`QuizQuestion = {"id", "quiz_id", "kind", "prompt", "options", "answer",
"explanation", "key_points", "chunk_id"}`. `kind` is `mcq | short`; `options` is
four strings for an `mcq` and null for a `short`; `key_points` is the reverse.
`chunk_id` is null when the model cited a label that was not in the prompt's
context: the question is kept and only its link is lost (A17).

### `POST /workspaces/:id/quizzes`

Auth, and the email must be verified: generation is one of the four actions A43
keeps locked until then, so an unverified account gets 403 `email_not_verified`.

Body: `{"title": string, "source_ids": [uuid], "count": number, "difficulty":
"easy"|"medium"|"hard", "types": "mcq"|"short"|"mixed"}`. `count` is how many
questions to ask for; the generator may return fewer and is told never to pad.
`difficulty` and `types` are required rather than defaulted: a default would make
"mixed, medium" the silent meaning of an absent setting, and what the student got
would differ from what they asked for.

201 `{"quiz": {"id", "workspace_id", "title", "difficulty", "question_types",
"source_ids", "status": "generating", "error": null, "requested_count",
"question_count": 0, "created_at"}}`. The body is the same `Quiz` shape every
other route serialises, so it cannot drift from `GET /quizzes/:id`.

- 400 `validation` when the title is empty or over 120 characters, `count` is
  outside `1..=50`, `difficulty` or `types` is not one of the values the prompt
  documents, `source_ids` is empty or longer than 20, or a source is not this
  workspace's, not this user's, or not `ready`.
- 402 `insufficient_credits` when the cost exceeds the balance. The cost is
  `ceil(count / 10) × quiz_10`, i.e. **6 credits per 10 requested questions** (21
  questions is three batches). Quoted from the *requested* count, so a request
  larger than the generator will produce is refused rather than truncated and the
  quote can never overcharge (A96).
- 422 `limit_reached` when the request would cross the plan's `quizzes_per_month`,
  counted over the user's **local calendar month**. `quizzes_per_month` counts
  **quizzes, not questions** (A94): free is 2, and counting questions would make a
  single ten-question quiz exceed it. The count includes the quizzes still
  generating, so a burst of concurrent requests cannot each pass on the last slot;
  the check, the charge and the quiz row are one transaction for the same reason.
  The body carries `limit` and `used`.
- 403 `email_not_verified`.

Every check that can fail runs before the credit is spent, so a refused request
costs nothing.

### `GET /workspaces/:id/quizzes`

Auth. 200 `{"quizzes": [Quiz], "cost": {"per_batch", "batch_size",
"max_questions"}}`, newest first, tombstones excluded. `cost` is the credit price
descriptor the studio panel renders — `per_batch` is the plan matrix's `quiz_10`,
`batch_size` the number of questions one batch covers, and `max_questions` the
request cap the POST enforces — so the client never quotes a hardcoded price or
limit. A client that ignores `cost` still reads `quizzes` unchanged. **This route
is a deliberate addition beyond the spec's route list** (A95), for the same reason
the deck list is.

### `GET /quizzes/:id`

Auth. 200 `{"quiz": Quiz, "questions": [QuizQuestion]}`, ordered by the model's
own `idx`, which is stored rather than re-derived: every question of one
generation is inserted in one transaction, so they share a `created_at` to the
microsecond and the order would otherwise fall through to a random uuid.

Each question carries **the student's latest attempt** as `attempt`, or null when
it has none. This is what makes a quiz resumable: a reload, or a second device,
picks the quiz up where it was left rather than at question 1. A graded attempt
supersedes an ungraded one for the same question, and the order is total (grade,
then `answered_at`, then id), so this view and `GET /quizzes/:id/results` cannot
disagree. `attempt = {"id", "user_answer", "correct", "answered_at"}`, where a null
`correct` means answered but not yet judged.

Another account's quiz is 404, not 403: the id must not confirm that it exists.
`Quiz.status` is `generating | ready | failed`; a `failed` quiz carries a readable
`error` and its credits are refunded (A98).

### `POST /quizzes/:id/attempts`

Auth. Body: `{"question_id": uuid, "user_answer": string, "id": uuid?,
"answered_at": rfc3339?, "correct": bool?}`.

`answered_at` is the **device's** clock, and absent means the server's. An attempt
is one of the five kinds a client may write offline, so the moment the student
answered is the client's to report: stamping an offline answer with the server's
sync time would put it in the wrong place in their history and would make two
attempts land in the same instant. A timestamp more than 24 h in the future is
refused, because a device with a wrong clock would otherwise pin itself to the top
of every "latest attempt" read.

**The answer is the option *text*, never a letter or an index** (A93). The prompt
tells the model to copy the correct option verbatim into `answer`, so text is the
one representation both sides already agree on; accepting a letter would mean
guessing which option the client meant, and a wrong guess is a silently wrong
score.

`correct` is read **only for a short answer**. An MCQ is graded by the server,
which normalises both sides (trim, case-fold, collapse internal whitespace) and
compares them; a client that claims `correct: true` for a wrong option cannot
raise its own score (A90). For a short answer `correct` is the student's own
verdict and may be absent, which stores `NULL` and means "answered, not yet
judged" (A91).

`id` is client-generated, so a retry after a lost response cannot apply twice: the
insert is `on conflict (id) do nothing` and the response says whether it applied
(A92).

200 `{"applied": bool, "attempt": {"id", "question_id", "correct",
"answered_at"}}`. `correct` is `null` for an unmarked short answer.
- 400 `validation` when `user_answer` is empty, or `answered_at` is more than 24 h
  in the future.
- 404 when the quiz or the question is not this user's.

### `GET /quizzes/:id/results`

Auth. 200 `{"summary": {...}, "questions": [...]}`.

`summary = {"total", "answered", "graded", "correct", "score", "topics"}`.
`score` is `correct / graded`, or **`null` when nothing is graded yet**: "nothing
marked yet" and "everything wrong" are different facts and a client that renders
them the same is lying. `graded` counts only the attempts with a verdict, so an
unmarked short answer does not lower the score as the student works through the
quiz (A91).

`topics` is the weak-topic summary, grouped by `(source_id, page)` and sorted
**weakest first** by `correct / graded`, then title, page and id so the order is
total. A topic with nothing graded sorts after every assessed topic, because it is
unknown rather than weak. A question whose label never resolved appears in its own
bucket with a null `source_id` rather than being dropped: a summary that omitted
what it could not attribute would look better and mean less (A97).

`questions` is one entry per question with the student's latest attempt
(`user_answer`, `correct`, `answered_at`), where a graded attempt supersedes an
ungraded one for the same question: marking your own answer is the fact that
matters, and the two rows can share an `answered_at`.

Another account's quiz is 404.

## Reports (Phase 6)

Auth, and the email must be verified: a report generation spends credits, and A43
keeps every spending action behind a confirmed address. A report is generated in
**two steps**, and they are charged separately.

`ReportView`:

```json
{ "id": "uuid", "workspace_id": "uuid", "title": "Laporan Jaringan Komputer",
  "status": "draft", "error": null,
  "outline": { "title": "…", "sections": [
    { "heading": "Pendahuluan", "purpose": "…", "labels": [1], "gaps": false } ] },
  "body_md": "## Pendahuluan\n\n…", "sections": [
    { "heading": "Pendahuluan", "body_md": "…", "generating": false, "error": null } ],
  "created_at": "2026-10-12T07:00:00Z", "updated_at": "2026-10-12T07:00:00Z" }
```

`status` is `draft` (an outline exists, not every section is written),
`generating` (a section is in flight), `ready` (every section has text) or
`failed`. The client polls while it is `generating`, which is why that value is
never left behind: a section that finishes sets `draft` or `ready`, and a
permanent failure sets `failed`.

`outline` is the editable plan and is `null` only for a row written before this
phase. It is passed through as stored rather than re-derived, so a field the
student edited cannot be lost on the way out. `gaps: true` means the section's
topic is **not** supported by the sources — the spec's "Not covered in your
sources" flag, and the reason the field is required output rather than
decoration.

`sections` is one entry per outline section, in outline order, and is what the
screen renders: `body_md` is `null` until that section has been written (a
different state from written-and-empty, which is why it is nullable rather than
`""`), `generating` is true only for the section a job is writing, and `error`
carries a failed section's reason on that section rather than on the report.
`body_md` is the whole document: each written section under its heading, in
outline order.

- `POST /workspaces/:id/reports/outline` body
  `{ "topic", "style", "length", "source_ids": [uuid] }` → **201**
  `{ "report": "...ReportView" }` in status `draft`.

  Synchronous: one model call, then the outline is stored for the student to edit.
  There is deliberately no `generate_report_outline` job — the roadmap's job table
  is a fixed list and has no outline kind, and a call this short whose result is
  edited before anything else happens is the opposite of the long cancellable work
  the queue exists for.

  Costs `report_outline` (**3** credits). Charged once, at the start, inside the
  same transaction that writes the reserving row: a report that is still generating
  has no sections at all, so a cap counted from the sections would read zero and be
  exceeded by asking twice. 400 `validation` for an empty topic, an empty `style`
  or `length`, or more than 20 sources; 400 when a source is not this workspace's
  or has not finished indexing; 422 `limit_reached` with `min_plan`, `limit` and
  `used` when the plan's `reports_per_month` is reached; 402
  `insufficient_credits`; 502 `provider_error` when the model cannot produce an
  outline, in which case **the charge is refunded** and the report is left
  `failed`. 404 for another account's workspace.
- `GET /workspaces/:id/reports` → 200 `{ "reports": [ "...ReportView" ] }`, most
  recently updated first, tombstones excluded. Not in the spec's route list; added
  because without it a report is unreachable after a reload.
- `GET /reports/:id` → 200 `{ "report": "...ReportView" }`. 404 for another
  account's report (never 403: whether an id exists is not information this route
  leaks).
- `PATCH /reports/:id` body `{ "outline": { "title", "sections": [ … ] } }` → 200
  `{ "report": "...ReportView" }`. Replaces the outline wholesale rather than
  patching field by field: it is editable, so an edit the generator could not have
  produced is still valid input as long as it parses, and a field-level patch would
  need a second shape to describe a rename, a reorder and a delete.

  **Every section must carry `heading`, `purpose`, `labels` and `gaps`.** The body
  is validated by the same core contract the model's reply goes through, so an
  outline that drops `labels` — the chunk labels a section's claims cite — is 400
  `validation` rather than stored with a default. The SPA round-trips all four
  fields.

  The section texts follow their headings, and only when a heading is unambiguous:
  a reorder keeps each section's text, while a rename, a duplicate heading or a
  deletion leaves that section unwritten. The replaced `body_md` is written to
  `report_revisions` before it is overwritten, so the losing side of a two-device
  edit is recoverable (the roadmap's conflict rule). No charge. 400 `validation`
  for an outline that does not match the contract; 404 for another account's
  report.
- `POST /reports/:id/generate` body `{ "index": 0 }` → **202**
  `{ "report": "...ReportView" }` with that section in flight. The work is a
  `generate_report_section` job; poll `GET /reports/:id`.

  Costs `report_section` (**6** credits) **per section**, so a student can fix one
  section without paying for the whole report again. Regenerating an index replaces
  that section's text and nothing else. 400 `validation` for an index outside the
  outline; 409 `conflict` when the report already has a section in flight, which is
  what stops two devices rewriting `body_md` from rows they read before the other
  wrote; 402 `insufficient_credits`. A generation that fails for good refunds the
  charge exactly once and marks the report `failed` with a readable `error`.
- `GET /reports/:id/export?format=md|pdf` → the document as bytes, with
  `content-disposition: attachment`. `md` is `text/markdown; charset=utf-8` and is
  `body_md` verbatim; `pdf` is `application/pdf`, printed through the server's
  Chromium with the report's title as the running header and a `Halaman N dari M`
  footer, and is available only where a renderer is configured (502
  `provider_error` otherwise).

  Both check the plan's `export_formats` **before** anything is produced and answer
  **403 `plan_required`** with `min_plan` when the format is not included — free
  gets `[md]`, pro and power get `pdf`. 400 `validation` for an unknown format
  (a different remedy from an upgrade, so the two are not collapsed); 409
  `conflict` when no section has been written yet.

## Cheatsheets (Phase 6)

Auth, verified email, and credits. A cheatsheet is one strict-JSON generation
rendered through a fixed print template: the model writes **content**, the server
owns every tag, and the typed blocks are stored so the document can be re-rendered
later for free.

`CheatsheetView`:

```json
{ "id": "uuid", "workspace_id": "uuid", "title": "UTS Jaringan Komputer",
  "scope": "UTS Jaringan Komputer", "exam_date": "2026-11-01", "depth": "normal",
  "source_ids": ["uuid"], "status": "ready", "error": null,
  "template_version": "cheatsheet.v1", "page_count": 6,
  "created_at": "2026-10-12T07:00:00Z", "updated_at": "2026-10-12T07:00:00Z" }
```

`status` is `generating` while the model is writing, `ready` once the blocks and
the HTML are stored, and `failed` when the generation gave up. **A cheatsheet whose
PDF failed is still `ready`**: the blocks are the document, and a print is
retryable and free, so a browser problem must not discard a generation the student
paid for. The print's failure is reported in `error`, and `page_count` stays `null`
until a print exists — it is read back from the renderer rather than estimated,
because the footer's `Halaman N dari M` is printed from that same number.
`template_version` names which `templates/cheatsheet.*.html` produced the HTML, so
an old document is visible.

The internal object keys (`blocks_key`, `html_key`, `pdf_key`) are **not** part of
the payload: a client has no use for a storage key, and exposing one would invite a
direct-storage contract this API does not offer.

- `POST /workspaces/:id/cheatsheets` body
  `{ "scope", "exam_date"?, "depth", "source_ids": [uuid] }` → **201**
  `{ "cheatsheet": "...CheatsheetView" }` in status `generating`. `depth` is
  `brief`, `normal` or `thorough`; `exam_date` is `YYYY-MM-DD` or absent.

  Costs `report_section` (**6** credits) — the matrix has no `cheatsheet` key, and
  the roadmap says the report's section price is what a cheatsheet costs rather
  than a price invented here. Charged inside the same transaction that writes the
  reserving row, exactly as a quiz is. There is **no monthly cap**: the matrix has
  no cheatsheet allowance, and counting a cheatsheet against `reports_per_month`
  would invent a policy `config/plans.toml` does not state (A119). 400
  `validation` for an empty scope, an unknown depth, an empty or oversized
  `source_ids`, an exam date more than ten years out, or a source that is not this
  workspace's and finished indexing; 402 `insufficient_credits`; 404 for another
  account's workspace.
- `GET /workspaces/:id/cheatsheets` → 200 `{ "cheatsheets": [ "...CheatsheetView" ] }`,
  newest first. Not in the spec's route list; added for the same reason the quiz
  list was.
- `GET /cheatsheets/:id` → 200 `{ "cheatsheet": "...CheatsheetView" }`. 404 for
  another account's row.
- `GET /cheatsheets/:id/html` → **200 `text/html; charset=utf-8`**, the document
  itself and not a JSON envelope. The preview frame consumes it as `srcdoc`, and
  the free plan's remedy for a withheld PDF is to print this document from a
  browser — neither works through an envelope. Served with `cache-control:
  private, no-store` and a `content-security-policy` that allows no script, because
  the document contains text a model wrote. Not plan-gated: the HTML is the
  document the credits bought. 409 `conflict` before the generation has rendered
  it; 404 for another account's row.
- `GET /cheatsheets/:id/pdf` → **200 `application/pdf`**, plan-gated on
  `export_formats` exactly like the report export (403 `plan_required` with
  `min_plan`). 409 `conflict` when the document is ready but its print does not
  exist yet — the cheatsheet is there, and the remedy is a re-render, which is not
  what a 404 says. The plan check runs before the resource is read, which leaks
  nothing: a non-owner on a plan without `pdf` gets the same 403 an owner would.
- `POST /cheatsheets/:id/re-render` → 200 `{ "cheatsheet": "...CheatsheetView" }`.
  Reads the stored blocks, re-runs the **pure** renderer with the current template,
  updates `html_key` and `template_version`, and re-queues the print. It never
  calls the model and **costs nothing**: no charge, no ledger row, no usage event.
  That is the whole reason the blocks are persisted — a template fix reaches a
  document the student may already have printed, and re-generating through the
  model would cost credits *and* produce different content. 409 `conflict` when the
  row has no stored blocks; 404 for another account's row.

## Entitlements and usage (Phase 7)

Every limit, feature and price comes from `config/plans.toml`, loaded once at
startup. The decision is made per request from the caller's **effective** plan:
`users.plan_id` grants paid limits only while the `subscriptions` row's status is
`active` or `grace` (A146), so a stored plan with no live subscription is free.

### `GET /usage`

The single source of the numbers the plan and usage screen shows (A159). Every
value is computed — a limit from the loaded matrix, a `used` from the same
snapshot the decision uses, a cost from `credits::costs`, the one function that
builds the block, so the number this view reports and the number a route charges
cannot drift. `null` is never `0`: an axis with no running counter reports `used:
null`, and `limit: null` means unlimited.

```json
{ "plan": "free", "name": "Free",
  "subscription": { "status": "active", "period_end": "2026-11-11T00:00:00Z" },
  "credits": { "monthly": 60, "purchased": 0, "total": 60 },
  "storage_bytes": 1048576,
  "limits": {
    "workspaces":              { "limit": 2,     "used": 1 },
    "sources_total":           { "limit": 20,    "used": 1 },
    "max_file_mb":             { "limit": 20,    "used": null },
    "storage_mb":              { "limit": 200,   "used": null },
    "devices_max":             { "limit": 2,     "used": 1 },
    "credits_per_month":       { "limit": 60,    "used": null },
    "chat_msgs_per_day":       { "limit": 20,    "used": 0 },
    "web_searches_per_day":    { "limit": 3,     "used": 0 },
    "flashcards_per_month":    { "limit": 30,    "used": 0 },
    "quizzes_per_month":       { "limit": 2,     "used": 0 },
    "reports_per_month":       { "limit": 1,     "used": 0 },
    "syllabus_imports_total":  { "limit": 1,     "used": 0 },
    "google_accounts_max":     { "limit": 0,     "used": null } },
  "features": {
    "export_formats": ["md"], "task_ai_breakdown": false, "quick_capture": false,
    "google_calendar": false, "planner": "manual", "agent_actions": false,
    "reminder_channels": ["web", "push"], "model_tier": "cheapest",
    "byok": false, "advanced_analytics": false },
  "costs": {
    "chat_message": 1, "web_search": 2, "flashcard_batch_10": 4, "quiz_10": 6,
    "report_outline": 3, "report_section": 6, "task_breakdown": 2,
    "syllabus_import": 8, "plan_generation": 2, "agent_turn": 2,
    "long_doc_threshold_tokens": 20000, "long_doc_credits_per_1k_tokens": 1 },
  "provider_cost": { "paused": false, "resets_at": "2026-11-01T00:00:00Z" } }
```

`provider_cost` carries no dollar figure: the user is told *that* the
provider-cost ceiling is reached and when it resets, not our unit economics. The
dollars are admin-only (`GET /admin/margin`). `resets_at` is the first midnight
of the caller's own local month, expressed in UTC, so a `Asia/Jakarta` account
reads the date it will actually reset on.

### Every route, and what it is

The table below is the same table `crates/server/tests/route_sweep.rs` asserts,
so the document and the test cannot disagree. **53 rows are registered** and every
one is classified; a new route that is not in the table fails
`every_registered_route_is_classified`.

**Swept** — calls the engine for a `Feature`, and an account whose limits are all
used gets exactly the `(status, code)` in the last column:

| Method | Path | Feature | Refusal |
|---|---|---|---|
| POST | `/workspaces` | `CreateWorkspace` | 422 `limit_reached` |
| POST | `/workspaces/{id}/chat` | `ChatMessage`; `WebSearch` too when the turn ran a chargeable search | 422 `limit_reached` |
| POST | `/workspaces/{id}/decks` | `FlashcardBatch{n}` | 422 `limit_reached` |
| POST | `/workspaces/{id}/quizzes` | `CreateQuiz{n}` | 422 `limit_reached` |
| POST | `/workspaces/{id}/sources` | `UploadSource{size_bytes}` | 422 `limit_reached` |
| POST | `/workspaces/{id}/reports/outline` | `Report` | 422 `limit_reached` |
| POST | `/reports/{id}/generate` | credits only (`report_section`) | 402 `insufficient_credits` |
| GET | `/reports/{id}/export` | `ExportFormat{format}` | 403 `plan_required` |
| POST | `/workspaces/{id}/cheatsheets` | credits only (`report_section`) | 402 `insufficient_credits` |
| GET | `/cheatsheets/{id}/pdf` | `ExportFormat{Pdf}` | 403 `plan_required` |

**Read-only** — a read of the caller's own rows, or an action that spends
nothing. Never gated: the spec requires over-limit data to stay viewable and
exportable (A149).

| Method | Path | Why it is not swept |
|---|---|---|
| GET | `/auth/me` | reads the caller's own account row |
| POST | `/auth/logout-all` | revoking sessions frees a device slot |
| POST | `/auth/resend-verification` | must work while locked out by A43 |
| POST | `/auth/google/set-password` | an account action, not a plan feature |
| GET | `/devices` | reads the caller's own devices |
| PATCH | `/devices/{id}` | renaming a device spends nothing |
| DELETE | `/devices/{id}` | revoking a device is the remedy a device refusal names |
| PUT | `/devices/{id}/push-token` | registering a token is not a plan feature |
| GET | `/workspaces` | reads the caller's own workspaces |
| GET | `/workspaces/{id}` | reads the caller's own workspace |
| PATCH | `/workspaces/{id}` | renaming spends nothing |
| DELETE | `/workspaces/{id}` | deleting is the remedy a workspace refusal names |
| GET | `/chunks/{id}` | reads a chunk of the caller's source |
| GET | `/workspaces/{id}/messages` | reads the caller's own thread |
| GET | `/workspaces/{id}/decks` | reads the caller's own decks |
| GET | `/decks/{id}` | reads the caller's own deck |
| GET | `/decks/{id}/due` | reads the caller's own due cards |
| POST | `/flashcards/{id}/review` | a review is free (R1): studying never costs credits |
| GET | `/workspaces/{id}/quizzes` | reads the caller's own quizzes |
| GET | `/quizzes/{id}` | reads the caller's own quiz |
| POST | `/quizzes/{id}/attempts` | an attempt is free (R1), like a review |
| GET | `/quizzes/{id}/results` | reads the caller's own results |
| GET | `/workspaces/{id}/sources` | reads the caller's own sources |
| GET | `/sources/{id}/status` | polls ingestion, which is already paid for |
| GET | `/sources/{id}/file` | reads the caller's own file |
| GET | `/sources/{id}/preview` | reads the caller's own PDF rendition; a read, so an over-limit account keeps it (A149) |
| DELETE | `/sources/{id}` | deleting frees a slot, per the refusal's message |
| GET | `/workspaces/{id}/reports` | reads the caller's own reports |
| GET | `/reports/{id}` | reads the caller's own report |
| PATCH | `/reports/{id}` | editing an outline spends nothing |
| GET | `/cheatsheets/{id}` | reads the caller's own cheatsheet |
| GET | `/workspaces/{id}/cheatsheets` | reads the caller's own cheatsheets |
| GET | `/cheatsheets/{id}/html` | the HTML is the document the credits already bought; only the PDF is gated |
| POST | `/cheatsheets/{id}/re-render` | re-rendering is a pure function of stored blocks and charges nothing |
| GET | `/usage` | reads the caller's own plan, counters and cost table; no decision and nothing spent (A149/A159) |

**Exempt** — no authenticated user at all.

| Method | Path | Why |
|---|---|---|
| GET | `/health` | no user |
| POST | `/auth/signup` | credentials; a limit check here would refuse the request that could fix the account |
| POST | `/auth/login` | credentials; rate-limited by `ratelimit::auth_layer` |
| POST | `/auth/refresh` | credentials; holds no bearer token yet |
| POST | `/auth/logout` | credentials; holds a refresh token, not a session |
| POST | `/auth/verify-email` | credentials; the token is the authorisation |
| POST | `/auth/google/exchange` | a single-use ticket, not a bearer token |
| GET | `/auth/google/start` | a browser redirect to Google |
| GET | `/auth/google/callback` | Google's redirect back, not the user's |
| GET | `/objects/{id}` | an HMAC-signed URL whose whole point is that it works without a session |
| PATCH | `/admin/users/{id}/plan` | `AdminUser` only: the extractor answers 404 for a non-admin (A7), so there is no decision to sweep |
| POST | `/admin/users/{id}/credits` | `AdminUser` only: the extractor answers 404 for a non-admin (A7), so there is no decision to sweep |
| GET | `/admin/margin` | `AdminUser` only: the extractor answers 404 for a non-admin (A7), so there is no decision to sweep |

### The refusals

Every refusal is the standard five-key envelope
(`{code, message, min_plan, limit, used}`), and the fields are populated by the
variant that produced it:

| `code` | Status | Raised by | `min_plan` | `limit` / `used` |
|---|---|---|---|---|
| `limit_reached` | **422** | a counted limit at its maximum: `CreateWorkspace`, `ChatMessage`, `WebSearch`, `FlashcardBatch`, `CreateQuiz`, `Report`, `UploadSource`, `DeviceLogin` | the plan that lifts it, or `null` on the top plan, where the limit is a genuine maximum and nothing is for sale (A140) | the limit and the count against it, so the UI can say "3 of 3 devices" |
| `plan_required` | **403** | a feature a higher plan has: `ExportFormat`, `TaskBreakdown`, `QuickCapture`, `GoogleCalendar`, `PlanGeneration`, `AgentAction`, `Byok`, `ReminderChannel`, `AdvancedAnalytics` | the cheapest plan that actually grants the feature, from the matrix — never simply the next plan up | `null` / `null` |
| `feature_locked` | **403** | locked with no upgrade to offer: no plan grants the feature at all, or the calling device's platform is `ios`/`android` (`config/billing_policy.toml` forbids selling there, A143); also a device over `devices_max`, whose remedy is to sign another device out | `null` | `null` / `null` |
| `insufficient_credits` | **402** | the balance cannot pay for the action | the plan whose monthly allowance is higher, so a spent-out user is told what fixes it | `null` / `null` |
| `email_not_verified` | **403** | any action that spends provider money, until the address is confirmed (A43) | `null` | `null` / `null` |

`plan_required` and `feature_locked` are not synonyms: `plan_required` means a
purchase fixes it and the client may be told which plan; `feature_locked` means
no purchase does, so the client is offered none (A143). The platform in that
decision comes from `devices.platform`, never from the request body (A144).

## Admin (Phase 7)

Three routes, all `AdminUser`-only (a non-admin gets 404, not 403 — A7). The
admin surface is deliberately API-only until Phase 13, which adds the payment
routes, the manual mark/refund and the completed margin report.

- `PATCH /admin/users/:id/plan` body `{ "plan": "pro" }` → 200
  `{ "user": { "id", "plan_id" } }`. Writes the stored plan **and** a live
  `subscriptions` row, because `users.plan_id` alone grants nothing (A146). 400
  `validation` for an unknown plan id, 404 for an unknown user.
- `POST /admin/users/:id/credits` body
  `{ "credits": 100, "bucket": "monthly"|"purchased", "idempotency_key": "..." }`
  → 200 `{ "granted": bool, "bucket", "credits" }`. `bucket` is validated rather
  than defaulted, so a typo is a 400 and never a silent monthly grant; the row's
  `reason` is `admin`, and `period_key = "admin:<idempotency_key>"` against the
  ledger's partial unique index, so a second call with the same key is
  `granted: false` and writes nothing.
- `GET /admin/margin?from=&to=` → 200 `MarginReport`: `per_plan[]` with
  `provider_cost_usd`, `byok_cost_usd` and `ceiling_usd` (the plan's
  `provider_cost_ceiling_usd`, attached by the route through
  `credits::ceiling_usd`, the field's one reader), a `provider_cost` split of
  `free_usd`/`paid_usd`/`byok_usd`, and `revenue`/`conversion` — which are
  **`null`**, not `0`: nothing in this phase can compute them, and a `0` would
  read as a real figure. BYOK cost is its own column because a BYOK call spends
  the user's own key, so counting it would overstate our cost (A156). Defaults to
  the last 30 days; 400 when `from >= to`.

## Tasks (Phase 9)

Auth. A task is one of the six entities a client may write offline, so every field
here is client-writable and the REST routes are **not** a second write path: they
call the same `tasks` table triggers a push does, which is what makes the two
produce the same `change_log` entries and the same LWW outcome. Both write paths
run the same field rules (`validate_task`), and both tombstone a subtree through
the same walk (`tasks::tombstone_subtree`) — a second copy of either is how the
two paths stop agreeing.

`TaskView`:

```json
{ "id": "uuid", "workspace_id": null, "parent_id": null, "title": "Selesaikan tugas kripto",
  "notes": null, "due_at": "2026-10-16T16:59:00Z", "estimate_min": 90, "priority": 1,
  "status": "todo", "position": 0, "created_at": "2026-10-12T03:01:41Z",
  "updated_at": "2026-10-12T03:01:41Z" }
```

`priority` is `1` high / `2` normal / `3` low, and `2` is the documented "unset"
(A210). `status` is `todo` or `done`. `position` orders the subtasks of one parent
and is a normal client-writable field (A196): the order key is
`(due_at nulls last, priority, position, created_at, id)`, and the id is the last
component because a batch insert shares `created_at` to the microsecond — the
mistake that made Phase 5's quiz questions come back shuffled (A100).

- `GET /tasks?status=&workspace_id=&parent_id=&due_before=&due_after=&limit=` →
  200 `{ "tasks": [ "...TaskView" ] }`. Tombstones are excluded, `limit` defaults
  to 200 and is clamped to 1–500, and the order above is applied in SQL.
- `POST /tasks` body `{ "title", "id?", "workspace_id?", "parent_id?", "notes?",
  "due_at?", "estimate_min?", "priority?", "status?", "position?" }` → 201
  `{ "task": "...TaskView" }`. The client may supply `id` (A8), which is what lets
  a row be created offline and synced later. 400 `validation` for an empty or
  over-long title, a priority outside 1–3, a negative `position`, a task that is
  its own parent, an unknown `status`, or a field a client may not write; 409
  `conflict` when that id is already in use (checked **without** a `user_id`
  predicate, so the answer is the same for another account's row and the id — not
  the row — is the only thing disclosed); 400 `validation` when `parent_id` or
  `workspace_id` names a row that does not exist. A dangling reference is the
  body's fault, so it is the client's 400 rather than a 500 from the foreign key.
- `PATCH /tasks/:id` body = any subset of the create fields → 200
  `{ "task": "...TaskView" }`. Partial: a field the body does not name keeps its
  stored value, and a nullable one named as `null` is cleared. `deleted_at` is
  **not** REST-writable here — deleting is `DELETE`, and a restore is a sync
  concern — so a body naming it is ignored rather than honoured. 404 for another
  account's row or a tombstone.
- `DELETE /tasks/:id` → 204. Tombstones the task **and its live descendants**
  (A197) in one transaction, one `change_log` `delete` row per tombstoned row, and
  404 when nothing was live to delete.
- `POST /tasks/quick-capture` body `{ "text" }` → 200
  `{ "draft", "credits_spent", "credits_remaining", "prompt_version" }`.
- `POST /tasks/:id/breakdown` body `{}` → 200
  `{ "proposal", "credits_spent", "credits_remaining", "prompt_version" }`.

**The two generative routes write no task.** That is the contract, not an
implementation detail (A193, A195): a capture returns a draft and a break-down
returns a proposal, and the student confirms before anything is saved. The only
writer of a task row is `POST /tasks` (or a push), so "accept" is N ordinary
creates — one per kept subtask, with `parent_id` and `position` — and dismissing a
proposal or a draft costs nothing and writes nothing. The consequence is stated
rather than hidden: **a reload loses an unaccepted draft**, and that is what
"confirm" means.

`CaptureDraft` — the model's reply, normalised, never a row:

```json
{ "title": "Selesaikan tugas kripto", "notes": "Bab 4 dan latihannya",
  "due_at": "2026-10-16T16:59:00Z", "priority": 1, "estimate_min": 90, "warnings": [] }
```

`warnings` is one line per field the reply could not supply as asked, and the draft
still returns: an unreadable `due_at` becomes `null` with a warning naming the raw
string rather than a failed request, because the credits are already spent by then
(A194). A naive datetime is read in the student's zone **and** warned about, so the
assumption is visible rather than silent. An out-of-range `priority` falls back to
2 while an out-of-range `estimate_min` clamps to 1440, and an estimate of zero or
less becomes `null` — "no estimate" and "estimated at zero minutes" are different
claims (A210).

`Proposal` — likewise never a row:

```json
{ "subtasks": [ { "title": "Baca bab 4", "estimate_min": 30 } ],
  "total_estimate_min": 95, "warnings": [] }
```

The vector's index *is* the `position` the caller assigns on accept. `total_estimate_min`
is the model's own number and is shown only inside the "Suggested …" region; the
`estimate_min` that gets stored is whatever the student left after editing.

Both generative routes:

- Price: `2` credits, read from `config/plans.toml`'s `[credits] task_breakdown`.
  A quick capture is charged as `task_breakdown` because the matrix has no
  `quick_capture` key and the rule is that the closest stated action's price
  applies rather than one invented in code (A190).
- Gate: `quick_capture` / `task_ai_breakdown` from the plan matrix, checked
  **before** the charge, so a plan that does not grant the feature gets the refusal
  it can act on rather than a charge it cannot: 403 `plan_required` with
  `min_plan`.
- Both take a **verified** account: an unverified one is 403
  `email_not_verified`, which is the same rule every AI action carries (A43).
- 400 `validation` for empty text or text longer than 2000 characters, and 404 for
  a break-down of a task that is not the caller's (or is a tombstone).
- 402 `insufficient_credits` when the balance cannot cover the reservation.
- The reply carries `prompt_version` whether or not the reply was usable, because
  every model call is attributable (A38). A provider failure refunds the
  reservation rather than charging for nothing.

## Events (Phase 9)

Auth. An event is one of the six offline-writable entities. `kind` is one of
`class`, `exam`, `study_block`, `deadline`, `external`; a deadline is an event with
`kind = "deadline"` and `ends_at = starts_at`, because `events.ends_at` is
`not null` and there is no all-day flag (A199).

`EventView`:

```json
{ "id": "uuid", "workspace_id": null, "kind": "exam", "title": "UTS Kripto",
  "starts_at": "2026-10-16T00:00:00Z", "ends_at": "2026-10-16T00:00:00Z",
  "source": "app", "task_id": null, "created_at": "2026-10-12T03:03:13Z",
  "updated_at": "2026-10-12T03:03:13Z" }
```

- `GET /events?from=&to=` → 200 `{ "events": [ "...EventView" ] }`. The window is an
  **overlap**, not a containment: a row is in it when `starts_at < to` **and**
  `ends_at > from`. A `starts_at between from and to` filter drops the three-hour
  class that began before the window and is still running, which is the one event on
  that timeline the student most needs. `from` is required; `to` defaults to
  `from + 7 days`; 400 when `to` is before `from`. Ordered by `starts_at, id`.
- `POST /events` body `{ "kind", "title", "starts_at", "ends_at", "id?",
  `workspace_id?", "task_id?" }` → 201 `{ "event": "...EventView" }`. `source`
  defaults to `"app"`. 400 for an unknown `kind`, a missing required field, an
  unknown key, **any of the four calendar-owned columns**, or a `workspace_id` /
  `task_id` that does not exist; 409 when the id is taken.
- `PATCH /events/:id` body = any subset → 200 `{ "event": "...EventView" }`.
- `DELETE /events/:id` → 204.

**Four columns are the calendar sync's, not a client's** (A198): `source` (any
value other than `"app"`), `external_id`, `etag` and `connection_id`. Phase 10
fills them from Google and ICS, so a client that could write them could forge a
"Google" event, hijack the `(connection_id, external_id)` unique index, or
overwrite a synced row that the next incremental sync would then not match.
Through REST each is 400 `validation`; through `/sync/push` each is
`read_only_entity`, which is the same rule in the vocabulary each path's client
reads. A **null** counts as setting it: clearing `external_id` breaks that match
just as surely as forging one. `source = "app"` is the one permitted value, so a
client echoing the row it pulled is not refused.

A row that already carries a calendar `source` is **read-only**: `PATCH` and
`DELETE` both answer 403 `forbidden` with a message naming the calendar, rather
than 404 — the row exists and a 404 would read as "deleted". The check reads the
row under the same row lock as the write, so a Phase 10 sync landing between a
check and a write cannot be overwritten. The push path refuses the same two
operations with `read_only_entity`, because an edit that lands is an edit the next
incremental sync silently reverts.

## Goals and planner (Phase 11)

A goal is the anchor the planner schedules against: an exam or a deadline with a
date, plus the sources its topics come from. Every route here takes `AuthUser`
except the two that spend credits, which take `VerifiedUser` because A43 keeps a
spend behind a confirmed address.

| Method | Path | Status | Notes |
|---|---|---|---|
| `GET` | `/goals` | 200 `{goals}` | Live goals for the caller, tombstone-free. |
| `POST` | `/goals` | 201 `{goal}` | `{title, target_date, workspace_id, source_ids}`. |
| `GET` | `/goals/:id` | 200 `{goal}` / 404 | Another account's goal is a 404, never a 403. |
| `PATCH` | `/goals/:id` | 200 `{goal}` / 404 | Partial write. |
| `DELETE` | `/goals/:id` | 204 | A tombstone: the goal is synced, so the removal has to reach every device. Its blocks survive with `goal_id` null (`on delete set null`). |
| `GET` | `/goals/:id/progress` | 200 | `{goal_id, window_days, topic_coverage_pct, quiz_accuracy_pct, cards_due, weak_topics, topics}`. |
| `POST` | `/goals/:id/plan` | 202 `{proposal_id}` | Reserves `plan_generation` (2 credits) and enqueues `plan_goal`. |
| `GET` | `/plans/:id` | 200 `{proposal}` / 404 | The poll. |
| `POST` | `/plans/accept` | 200 `{event_ids}` | Writes the blocks. Spends nothing. |
| `POST` | `/plan/replan` | 202 `{proposal_id}` | Reserves 2 more credits. |

**`GET /goals/:id/progress` is the honesty surface.** There is no composite
readiness score and no grade prediction; `quiz_accuracy_pct` is `null`, never `0`,
when the goal has no attempts, because `0%` is a claim the app cannot make from an
empty set. `topic_coverage_pct` is `null` when the goal has no topics yet. `topics`
is the list the coverage is computed over, so a screen can show what the percentage
is a percentage *of*.

**Planning is a job, not a route** (A253). `POST /goals/:id/plan` reserves the two
credits, writes a `plan_proposals` row in status `planning`, enqueues the job and
answers 202 with `{proposal_id}`; `GET /plans/:id` is how a client polls. The
contrast with `create_outline` is deliberate: one short call may run in a route, a
whole-document call may not.

`GET /plans/:id` answers **200 in every status** — `planning`, `ready`, `failed`,
`accepted`, `superseded` — because a poll that turned a normal outcome into an HTTP
error would force every client to treat it as an exception before it could show the
message. The body is `{proposal: {id, goal_id, status, proposal, deficit_min,
warnings, prompt_version, error, created_at, updated_at}}`.

`proposal` is the pure module's `PlanProposal`: `blocks[]` (`starts_at`, `ends_at`,
`goal_id`, `topic`, `kind` ∈ `NewMaterial|Review`, `pinned`), `day_load[]`
(`date`, `planned_min`, `capacity_min`), `deficit_min`, and `warnings[]` — either
`{"Overloaded": {"days": [...]}}` or `{"PastDeadline": {"goal_id": …}}`, and the
enum is closed (A262). **`deficit_min` is a real computed number**: it is the
minutes of demand the horizon could not hold, and a plan that dropped the excess
silently would look complete and leave the student unprepared.

A **pinned** block arrives with `goal_id` `00000000-0000-0000-0000-000000000000`
and the topic `"Pinned"` (A265): contract § 6's `Interval` carries no goal
attribution. A client echoes such a block back **unchanged** — the server resolves
it to its committed event by the exact `(starts_at, ends_at)` range.

### Refusals

| Code | Status | When |
|---|---|---|
| `plan_required` | 403, `min_plan: "pro"` | The plan's `planner = "manual"`. Structural, not a copy decision: `Feature::PlanGeneration` is `planner != manual` (A257). |
| `insufficient_credits` | 402 | Not enough credits for the 2. |
| `validation` | 400 | `POST /plans/accept` naming the offending block index. |
| `conflict` | 409 | A second accept of one proposal, or a replan of a goal with no live proposal. |
| `not_found` | 404 | Another account's goal or proposal. |

**`POST /plans/accept` re-validates the client's blocks and writes the whole batch
or none of it** (A264). The client may drag a block; the server still checks the
merged result against busy time, the study window and `max_study_min_per_day`, and
answers 400 naming the block rather than writing part of a plan. The body is
`{proposal_id, blocks: [{starts_at, ends_at, goal_id, topic, kind}]}` with
`deny_unknown_fields`, and `kind` is a string: `"review"` case-insensitively for a
review-reserved block, anything else for new material. Accept spends nothing — the
two credits were charged at plan time.

A replan (A265) splits the goal's existing blocks: one that starts after `now` is
**pinned** and kept exactly; one that has already started joins the busy set,
because a replan must not schedule new material over a session the student is
sitting in.

## Syllabus (Phase 11)

| Method | Path | Status | Notes |
|---|---|---|---|
| `POST` | `/workspaces/:id/syllabus` | 202 `{draft_id, source_id}` | Multipart: `file` or `text`, optional `title`. `VerifiedUser`. Charges `syllabus_import` (8). |
| `GET` | `/syllabus/:id/draft` | 200 / 409 | The extraction and the review list. 409 `conflict` while it is still being read. |
| `POST` | `/syllabus/:id/confirm` | 200 `{draft_id, status, events, tasks, goals}` | The only writer of the rows. 409 on a second confirm. |

**Nothing reaches `events`, `tasks` or `goals` until the student confirms.** The
extraction is a model's reading of a document, and a wrong reading written straight
through is the failure the review step exists to prevent. So the upload writes a
`sources` row with `kind = 'syllabus'` and enqueues `ingest_source`, which gains one
step for that kind: compose `syllabus.v1.md`, call the model, write
`syllabus_drafts` (A250 — no new job kind is added, because the roadmap's § 8 list is
fixed). The extraction is idempotent: the draft is keyed uniquely on `source_id`, so
a retry updates the same row rather than appending a second one. A parse failure is
permanent; a model or transport failure is transient and retried once with a
corrective turn.

The draft body is `{draft_id, source_id, status, error, course, items[]}`, where each
item is `{index, kind: "class"|"deadline"|"exam", title, evidence, needs_check, when,
rrule, start_time, end_time, topics, weight_percent}`.

**There is no `confidence` field, and that is the point (A261).** The model returns a
score per item and the honesty rules forbid showing one, so the score is used to
**order** the list (lowest first, where the student's attention is) and to set
`needs_check` below `REVIEW_FLAG_CONFIDENCE = 0.6`; what the screen shows is the
**evidence quote** — the exact source line the item was read from — because a quoted
line is what actually lets a student verify the item. An empty quote is a parse
failure and a retry, not a weaker citation.

`status` is `pending | extracting | ready | failed | confirmed`. A `failed`
extraction is **200** with `status: "failed"` and the error in the body rather than a
4xx: this endpoint's job is to report the draft's state.

The confirm body is `{items: [{index, keep, title?, when?, rrule?, topics?,
source_ids?}]}`, `deny_unknown_fields`. An absent field means **"keep what the
extraction said"** rather than "clear it". An index that is not in the draft is a
400, and so is one that appears twice — both checked before anything is written, so
a malformed body cannot produce a half-written confirm. One transaction: the draft
must be `ready` and the caller's, the status becomes `confirmed`, and every kept
item's rows are written together.

The confirm mapping is fixed (A269): a kept meeting → one `events` row per expanded
occurrence (`kind='class'`, `source='app'`, expanded with Phase 10's
`calendar::expand` and capped at `MAX_OCCURRENCES = 60`); a kept deadline → one
`tasks` row (`due_at` set, `priority` 1, `estimate_min` null); a kept exam → one
`goals` row (`target_date` set, `source_ids` the workspace sources the student
selected, or every live source when they selected none).

**Free is capped by count, not refused.** `syllabus_imports_total = 1`, so the first
import is granted and the second is 422 `limit_reached`. The count and the charge
commit with the reserving row, so a failure rolls the charge back with it, and a
permanent extraction failure refunds the eight credits (A253).

## Calendar (Phase 10)

Auth, except the feed. Two directions of the same idea: the app's own events go out as an
ICS feed a student subscribes to, and other people's calendars come in as read-only rows.

**The plan split is the thing to get right.** The matrix's `ICS feed + import` row is on
**every** plan — free has the feed, the token, the ICS URL field and the import — while the
**Google connection is pro-only** (`google_calendar = false` on free, `google_accounts_max`
0/1/unlimited). So the only refusal in this section is on the Google routes and on
`POST /calendar/sync`; a `LockedFeature` around the feed or the ICS import would refuse a
feature free actually has.

### The feed

- `GET /cal/<token>.ics` → 200 `text/calendar; charset=utf-8`. **No bearer token**: the path
  segment is the credential (A242). The router template is `/cal/{token}` and the handler
  requires the `.ics` suffix, because `matchit` 0.8 rejects a static suffix after a route
  parameter at insert time — a panic in `router()`, not a 404 — so `/cal/<token>` without the
  suffix is a 404 rather than a second address for the same resource. An unknown token is
  also 404, from an index hit or a miss, never a 500.
- `Content-Type: text/calendar; charset=utf-8`, `Cache-Control: no-store`, and
  `X-Robots-Tag: noindex` (A241/A242). `no-store` because the feed is a cache the *client*
  owns and a stale copy served to a revoked token is the leak rotation exists to close;
  `noindex` because a subscription URL is exactly the thing a crawler finds through a shared
  link, a referrer or a chat preview.
- **What it carries** (A241): `source = 'app'`, not tombstoned, `kind in (class, exam,
  study_block, deadline)`, plus one synthetic all-day `deadline` per task with a `due_at` —
  the spec's "tasks with due dates, study blocks, and exams". `kind = 'external'` and every
  `source != 'app'` row are **excluded**: echoing them would show every Google event twice in
  the student's own client, under a different `UID`, with no way to tell the two apart.
- Each `VEVENT`'s `UID` is `<events.id>@stvdy.app` and its `SEQUENCE` is derived from
  `(created_at, updated_at)`, with `DTSTAMP` and `LAST-MODIFIED` from `updated_at` (A230). Two
  renders of an unchanged row are byte-identical, and an edit strictly raises `SEQUENCE`. A
  client that caches on `UID` and finds no `SEQUENCE` has no signal an event changed, so an
  edited title would be invisible forever, per client. *Stated limitation:* two edits inside
  one second produce the same `SEQUENCE`, because both are second-granular.
- A **deadline is an all-day `VEVENT`** (`DTSTART;VALUE=DATE:` and `DTEND;VALUE=DATE:` at the
  local date and the next local date) in the student's IANA zone from `users.timezone` through
  `chrono-tz`, never UTC (A248). Every other kind is a timed event with `DTSTART`/`DTEND` in
  UTC. A zero-length timed event is rejected or drawn as a point by most clients, and "due
  Friday" is what a date-only event means.
- Ordered by `starts_at` then `id`, so two renders of an unchanged set are byte-identical.
- **Rate-limited in the credential bucket** (30/min/IP) and `Exempt` in the route sweep,
  which is `Class::Exempt`'s own definition ("no authenticated user at all"). The route logs
  the resolved `user_id` and the byte length and **never the token** (A243) — a secret in a
  `tracing` field is a secret in every log aggregator the deployment has.

### The feed token

- `GET /calendar/ics-token` → 200 `{ "ics_feed_token": "..." | null }`. Not one of the spec's
  seven routes and required anyway: A242 says the token is "readable again by the settings
  screen", and a screen that could only obtain the link by *rotating* it would break every
  subscribed device each time the student opened settings. `null` means none has been
  generated, and is rendered as that rather than minting one on read. `ReadOnly` in the sweep.
- `POST /calendar/ics-token/regenerate` → 200 `{ "ics_feed_token": "..." }`. 32 fresh random
  bytes, URL-safe base64 with no padding. Rotation is revoke-and-replace in one statement, so
  the old value is overwritten and a client holding the previous URL 404s on its next poll.
  That is the whole remedy A242 promises, and it is why the token is a column rather than a
  signed value: a signed feed URL could not be revoked without rotating a server-wide key.
- The token is stored in **plaintext** in `users.ics_feed_token`, and that is a threat-model
  row rather than a silent choice (A242, `docs/THREAT_MODEL.md` § 2 and R7). A hash — the rule
  for refresh tokens and login tickets (A53) — cannot be shown again.

### The ICS import

- `POST /calendar/ics` body `{ "url" }` → 201 `{ "connection": "...ConnectionView" }`. The URL
  is validated **before the row is written**: 400 `validation` with a readable reason when it
  is not `http`/`https` or carries credentials, decided without DNS (A246). The fetch path
  guards again (A233) — the two do different jobs. This one is for the user ("that is not a URL
  we can fetch" beats a connection that silently never syncs); the fetch-time guard is the
  protection, because DNS can change between the two.
- 409 `conflict` when the same URL is already connected to this account. More than one ICS
  connection is allowed — a student can have two faculties — but two rows for one feed would
  import every class twice with no way to tell which row to delete.
- **The first import is a job**, so the reply is the connection and not the events: the
  enqueue is `sync_calendar` with the windowed dedupe key (A238).
- The fetch is a **second function on the shared SSRF guard**, not a second guard
  (`integrations::fetch::fetch_guarded_text`): `parse_target` + `resolve_public` + the same
  pinned-address client and the same redirect re-check, **skipping readability** (A233).
  `ChainFetcher` is not the entry point for a feed: `extract_readable` is right for a web page
  and destroys an ICS body. A consequence, stated: a route test cannot point the real fetcher
  at a local stub — the guard is precisely what refuses `127.0.0.1` — so the importer takes a
  fetcher seam, the route tests inject a fake, and the refusal is proven in
  `crates/server/tests/ssrf.rs` against `fetch_guarded_text` itself.
- **Conditional GET** (A239): `ics_etag`/`ics_last_modified` hold the validators and the fetch
  sends `If-None-Match`/`If-Modified-Since`. A `304` advances `last_synced_at` only. The
  expensive half is not the download: an unconditional re-upsert touches every row, and
  `events` is `+sync`, so every touched row writes a `change_log` entry and every device pulls
  the whole calendar every poll.
- **An occurrence, not a series, is a row** (A231). `events.external_id` holds the occurrence
  key — Google's opaque event id verbatim, and for ICS `"<UID>|<recurrence-id in UTC>"`, with
  `"<UID>"` alone for a non-recurring one — and the existing partial unique index on
  `(connection_id, external_id)` is the upsert key, so a re-poll is idempotent.
- **Expansion is bounded twice** (A232): `MAX_OCCURRENCES = 500` and `HORIZON_DAYS = 180`. The
  set asks for **one more than it keeps** and reports `truncated` when more were available,
  because `rrule`'s `collect_with_error` ends with `was_limited || limit == list.len()` — so
  asking for exactly the cap reports a rule with exactly that many occurrences as truncated,
  which is a connection that looks broken while it is fine. A truncated expansion writes
  `calendar_connections.error = 'recurrence_truncated'` and **keeps every occurrence it
  read**; the server reads the `Expansion.truncated` flag and never `occurrences.len()` (A247).
- `GET /calendar/connections` → 200 `{ "connections": [...ConnectionView] }`. An addition
  beyond the spec's route list (A249): the spec lists connect/callback/ics/delete/sync/
  regenerate/feed and no list, so a connection made in one page load would be invisible in the
  next and the settings screen could not show its status — the same reasoning as A81. **Never**
  a token of any kind, and no `ics_url`.
- `DELETE /calendar/connections/:id` → 204. The connection's events are **tombstoned in the
  same transaction**: `events.connection_id` is `references calendar_connections on delete set
  null`, so deleting the connection alone would leave orphan `source = 'ics'` rows that no sync
  will ever touch again and that a client cannot edit or delete either (they are read-only by
  A198) — a timetable the student cannot remove. A tombstone rather than a hard delete,
  because `events` is `+sync`: a row removed outright leaves every device showing the event
  forever, with no change-log entry to correct it. `ReadOnly` in the sweep, because an ICS
  connection is available on free and deleting one is the remedy a refusal names.

`ConnectionView` (A249):

```json
{ "id": "uuid", "provider": "ics", "account_email": null, "status": "connected",
  "error": null, "last_synced_at": "2026-10-12T03:00:00Z", "study_calendar_id": null }
```

`error` is the connection's own stored reason, rendered beside the connection rather than
swallowed: the screen must not claim a connection is healthy while it holds an error. It is
cleared by the next clean pass.

### The sync and the consent flow

- `POST /calendar/sync` → **202** `{ "queued": n }`. `n` counts the rows actually inserted, so
  a second press inside the same minute answers `0`. The dedupe key is
  `'sync_calendar:<connection_id>:manual:<floor(now/60)>'`, which bounds a user's own retry
  loop without a second rate limiter; the periodic pass uses the 900-second window instead
  (A238). Running the sync inline would hold a request open across several Google round trips.
  `Swept` in the sweep: `GoogleCalendar`, 403 `plan_required` on free.
- A **`410 Gone` is never surfaced as an HTTP status to the client**, because the client is not
  the one holding the token. Google answers it inside the sync; the server clears `sync_token`
  so the **next** pass is a full listing, and records the mode in
  `calendar_connections.last_sync_was_full` (A236). "An incremental sync that silently stops
  being incremental" is the phase's third named risk and it is invisible — every poll succeeds,
  the events stop changing, nothing errors — so the mode is a value a test asserts rather than
  an assumption.
- `GET /calendar/google/connect` → 200 `{ "url" }`, the Google consent URL for the SPA to
  open with `platform.openExternal` — the **system browser, never an embedded webview**,
  because a page this app draws around someone else's login form is exactly what the spec
  forbids. It requires `Feature::GoogleCalendar` and then `Feature::GoogleAccount` through the
  entitlement engine (A245) — the second is the counted arm over `google_accounts_max`, not a
  second read of the plan — in that order, so a free account is told the feature is not in
  their plan rather than "0 of 0 Google accounts". It answers **404** when
  `GOOGLE_CALENDAR_CLIENT_ID`/`_SECRET`/`_REDIRECT_URI` are unset, so a half-configured client
  never produces a redirect that fails confusingly at Google. `Swept` in the sweep: 403
  `plan_required` on free.
- `GET /calendar/google/callback` → **302** to
  `stvdy://oauth/calendar?status=connected|denied|error&reason=...`, the same construction and
  the same status as `GET /auth/google/callback`. **It redirects in every case, including its
  refusals**: the browser is sitting on Google's redirect URI, and a JSON error page there is
  a dead end the student cannot act on. `Exempt` in the sweep (a browser redirect carries no
  bearer token). **No token of any kind travels in the deep link** (A235/A48): the refresh
  token is written server-side at the callback and a deep link lands in browser history,
  referrer logs and the OS URL handler. The SPA's `DeepLinkHandler` refreshes the connections
  query on `status=connected` and reports the reason on `denied`/`error`. A `state` minted by
  the *login* flow is refused here by the payload's `purpose` (A235), and a tampered or
  expired one by the same `decrypt_state` the login callback uses.
- **Task 6's live half is not exercised by any test in this build**, because the OAuth client
  does not exist: the code exchange against Google, `access_type=offline` actually yielding a
  refresh token, and the "Study Plan" calendar's find-or-create (A237 — the row is written
  with `study_calendar_id = null` and the first sync fills it in, which is Task 7's
  `CalendarApi`). The plan's Task 6 records these four items by name.
- **The consent is a two-scope pair, chosen so the conflict policy is structural** (A234):
  `https://www.googleapis.com/auth/calendar.app.created` **plus**
  `https://www.googleapis.com/auth/calendar.events.readonly`, with `access_type=offline`,
  `prompt=consent` and PKCE (`S256`). `calendar.app.created` can create and manage only
  calendars this app made — which is "our study blocks go to a dedicated 'Study Plan'
  calendar" (A237) — and `calendar.events.readonly` can read everything else and change
  nothing. So "Google wins for events we didn't create" is enforced by the token rather than by
  remembering a rule, and a bug in our merge logic cannot rewrite a user's own event. Login is
  a **different** OAuth client with different scopes and a different redirect URI; sharing one
  would mean the login screen asking for calendar write access. The encrypted `state` carries a
  `purpose` (`Login` | `Calendar`), which is what keeps a login state from being replayed as a
  calendar state — the two flows share a codec, not a trust domain (A235).

### Read-only, on both write paths

A row the sync wrote is read-only to a client on both paths (A240). The rule lives in the
routes and the push validator, **never in a trigger**: the ICS importer and the Google sync
write `source`, `external_id`, `etag` and `connection_id` through plain SQL, so a trigger that
refused the write would refuse the sync's own write, and the only way to keep the two apart
would be a session variable — a second mechanism for a distinction the two paths already make
structurally. `crates/server/tests/calendar.rs` asserts the non-obvious half: a row the sync
wrote is refused by `PATCH`, and the sync can still write it.

## Actions (Phase 12)

`GET /actions/pending`, `POST /actions/:id/confirm`, `POST /actions/:id/reject`.

A **WRITE tool never executes on its own** (A271). The gate has no arm that can run one, so a
`create_task` the model emits becomes a row in `pending_actions` plus an `action` event on the
chat stream, and the only thing that turns that row into a domain row is the confirm below.

### `GET /actions/pending`

Auth. Runs the lazy expiry first (A274), then answers the caller's proposals:

```json
{ "actions": [ { "id", "tool", "args", "status", "created_at", "expires_at",
                 "resolved_at", "resolved_by_device_id", "result", "error",
                 "message_id" } ] }
```

`status` is the server's and is one of `pending`, `confirmed`, `rejected`, `expired`. The list
carries the **pending** rows plus every row resolved in the last `RESOLVED_VISIBLE_HOURS` (24)
— which is what lets a card confirmed on device A read "Confirmed" on device B rather than
vanish — newest first, `limit 50`.

The lazy expiry is one `update … set status = 'expired'` before the select. A `GET` that
writes is deliberate: the alternative is a scheduler pass over every account's pending actions
and the roadmap § 8 job-kind list is fixed with no expiry kind, and the claim re-checks
`expires_at` inside its own transaction, so this is the convenience and not the enforcement.

### `POST /actions/:id/confirm`

Auth, `Feature::AgentAction` → `403 plan_required`, checked **before the row is read** — so an
account without the entitlement is refused whatever the id names.

Body: `{ "args": { … } }`, optional and `deny_unknown_fields`. Absent confirms exactly what was
proposed; present is the Edit half of Confirm/Edit/Dismiss and is re-parsed through the same
`parse_args` the proposal went through, so an edit can only ever **narrow** what the model
proposed — the tool's bounds, the field rules and the closed `additionalProperties` all apply
again. An invalid edit is `400` and the row stays `pending`.

Order is the contract: plan → row → claim → execute → record → commit. The claim is a guarded
state flip inside the execution's own transaction (A275):

```sql
update pending_actions set status = 'confirmed', resolved_at = now(),
       resolved_by_device_id = $3
 where id = $1 and user_id = $2 and status = 'pending' and expires_at > now()
 returning id
```

**Zero rows is `409 conflict`**, with a body naming the current status — so a second tap, on
either device, writes nothing and the client's remedy is to re-read the row. The executor runs
inside that transaction, so a failure rolls the claim back and a `confirmed` row can never be a
row that did nothing. `resolved_by_device_id` is what makes "confirm on device B" a fact rather
than a claim.

Another account's proposal is `404`, never `403`: a `403` would confirm the id exists.

A confirmed `create_flashcards` or `create_quiz` charges `flashcard_batch_10` / `quiz_10` on
the deck and quiz paths' own spend — the executor calls those routes' service functions, so
there is one implementation of "create a deck" and one price for it (A276).

The one failure the rollback cannot cover is the enqueue **after** the commit: the row *was*
written and the job that generates it could not be queued. That is recorded on the row as
`error` and answered as a failure, so the card on every device says why the work did not start
rather than claiming a deck that is not generating.

### `POST /actions/:id/reject`

Auth. Deliberately **not** gated by the plan: it spends nothing and frees the student from a
card, and gating it would leave a downgraded account unable to dismiss a proposal it can no
longer confirm. Single-winner like the confirm, and for the same reason — two devices
dismissing the same card must agree about what happened, so the second is told `409` rather
than silently succeeding. `404` for another account's row.

## Sessions and activity (Phase 9)

Auth. A study session is append-only and client-id'd: the timer mints the id when it
**starts**, persists it with the running timer, and flushes it when it stops (A206),
so a flush after a restart is the same id and `on conflict (id) do nothing` makes a
replay a no-op. That is the mechanical defence against "a timer that logs a session
twice on restart" — and it is why the id, not the row count, is what the tests
assert.

`SessionView`:

```json
{ "id": "uuid", "workspace_id": null, "task_id": "uuid",
  "started_at": "2026-10-12T02:15:26Z", "duration_min": 50,
  "created_at": "2026-10-12T03:05:26Z", "updated_at": "2026-10-12T03:05:26Z" }
```

- `POST /sessions` body `{ "id", "started_at", "duration_min", "client_ts",
  "task_id?", "workspace_id?" }` → 201 `{ "applied": true }`, or
  `{ "applied": false }` when that id already had a row. `client_ts` is the
  **device's own report of when it wrote the row** and is required: it is the field
  the bound is compared against, and the mutation contract already carries it
  (`Mutation.client_ts`), which is what lets one validator serve both paths. A body
  that also names `client_ts` in the payload must agree with the value the caller
  passed.
- `GET /sessions?from=&to=` → 200 `{ "sessions": [ "...SessionView" ] }`. `from` is
  required, `to` defaults to `from + 1 day`, and the order is `started_at desc,
  id desc`. 400 when `to` is before `from`.
- `GET /activity?from=&to=` → 200 `{ "reviews": [...], "attempts": [...],
  "sessions": [...] }` — three arrays of instants, in one round trip.

**The plausibility bound, and why it reads the client's clock** (A204):

| Rule | Value |
|---|---|
| `duration_min` | `1 ..= 1440` |
| `started_at` | not more than 24 h past `server_now` — the same bound a review and an attempt already use |
| `started_at + duration_min` | not more than `client_ts + 2 min` |

Comparing the last one against the *server's* clock would reject a legitimate
offline session from a device whose clock runs fast, and rejecting valid data is its
own defect. A violation is 400 `validation` with a message saying what it compared,
and it writes nothing. `docs/THREAT_MODEL.md` § 6 R1 is the risk this narrows: the
rows stay client-writable, but a fabricated one can no longer claim more time than
the device said had elapsed.

`GET /activity` exists because the web build has no local cache (A183) and the
streak is computed on the client (A205): the route ships the three instants and the
*rule* that turns them into a streak lives in
`app/src/features/today/day.ts`, because the Today view has to work offline and a
server route cannot. A second implementation here would be a second answer to "how
many days in a row".

## Settings (Phase 9)

Auth. The settings row is keyed by the user and **created on first read**, so a read
never has to answer "no settings yet" and a write never has to decide whether to
insert. `GET /settings` returns the row and the account's timezone *beside* it:
the timezone lives on `users` (A203) because the reminder and planner maths read it
for an account that has never opened this screen, and a second copy on the settings
row is exactly the drift the "local-date maths only through `chrono-tz`" rule would
not catch.

```json
{ "settings": { "study_start_hour": 8, "study_end_hour": 22,
    "max_study_min_per_day": 240, "block_len_min": 50, "break_min": 10,
    "reminder_channels": ["web"], "quiet_start_hour": 22, "quiet_end_hour": 7,
    "language": "id", "theme": "system", "updated_at": "2026-10-12T03:02:05Z" },
  "timezone": "Asia/Jakarta",
  "channels_available": ["web", "push", "email"] }
```

**`channels_available` is what this deployment can actually deliver on** (A288), and
it is a different question from what the plan grants. `telegram` and `discord` are
in `config/plans.toml`, `PUT /settings` keeps accepting them for Power, and no
transport sends them — so the field is derived from the transports that exist:
`web` and `email` unconditionally (the in-app feed and the desktop shell's local
notification need no transport, and the mailer is always present because a
deployment with no SMTP credentials gets the dev sink), and `push` only when
APNs/FCM credentials are configured. The order is the matrix's own. The settings
screen offers only what appears here, because the alternative — a channel that is
granted, selectable and silently never fires — leaves the student's only evidence a
reminder that never arrived. A delivery on a channel with no transport is recorded
with `reason = 'channel_not_implemented'` and logged at `warn`.

- `GET /settings` → 200 the shape above.
- `PUT /settings` body = any subset of `{ "study_start_hour", "study_end_hour",
  "max_study_min_per_day", "block_len_min", "break_min", "reminder_channels",
  "quiet_start_hour", "quiet_end_hour", "language", "theme", "timezone" }` → 200
  the same shape.

`PUT` is **partial**: the screen saves per section, and a full replace would let a
stale tab clobber a channel set changed on another device. What it writes is the
whole row, because the roadmap gives `user_settings` record-level LWW and the table
has no per-field timestamp to merge with (A201).

Bounds, each 400 `validation` before the write, so the migration's `check`
constraints are a backstop rather than the error path:

| Field | Rule |
|---|---|
| `study_start_hour` | `0 ..= 23`, and below `study_end_hour` — checked only when the body names both, because a partial write that changes one hour is a legitimate request |
| `study_end_hour` | `1 ..= 24` |
| `max_study_min_per_day` | `15 ..= 1440` |
| `block_len_min` | `5 ..= 180` |
| `break_min` | `0 ..= 60` |
| `quiet_start_hour`, `quiet_end_hour` | `0 ..= 23` each, and **a pair**: set both or neither, because a body naming one hour has stated half a window |
| `language` | `en` or `id` |
| `theme` | `system`, `light` or `dark` |
| `timezone` | an IANA zone `chrono-tz` can parse |

**The quiet window may wrap midnight.** The test is "inside the window", not
`start < end`, because `22 → 7` is the normal case (A202) — a naive `start < end`
check rejects the value every student actually wants.

**A reminder channel the plan does not grant is refused per channel**, 403
`plan_required` with `min_plan`, **before** the write, so a rejected channel leaves
the row exactly as it was. The check goes through
`entitlements::require(Feature::ReminderChannel(c))` and never a
`plans.get(..).reminder_channels` read: the matrix is the authority, a second read of
it here would be a second source of truth for what a plan grants, and the route
sweep forbids it (A201). `reminder_channels` is a **whole-array** write — the body
states the full set it wants, not a delta.

An unknown zone is refused rather than stored: `chrono-tz` reads an unparseable
string as UTC, so a typo would silently move the student's day boundary to
Greenwich and every date on the Today view with it (A6).

`PUT /settings/api-key` and `DELETE /settings/api-key` are **Phase 13** (BYOK): the
screen names them and they are not built here.

## Reminders (Phase 12)

`POST /reminders`, `GET /reminders`, `POST /reminders/:id/ack`.

**Registration is the client's, and it is idempotent by the dedupe key.** The spec says
clients may schedule local notifications from synced events and tasks so reminders work
offline; this route is what tells the server the same reminder exists.

```json
POST /reminders  { "kind": "task_due" | "event_soon", "subject_id": "<uuid>",
                   "fire_at": "2026-10-16T15:59:00Z" }
             →   { "applied": true, "reminder_id": "<uuid>" }        (201 / 200)
```

`cards_due` is **not** registrable: it is day-granular and derived from the deck, so a client
has nothing to compute it from that the server does not already have. The subject must be the
caller's own and its kind must match the payload's — a `task_due` naming an event id would
register a reminder that fires at the wrong time and is attributed to the wrong row, which no
later layer can detect — so a foreign or mismatched subject is `404`.

`fire_at` is bounded in both directions, each with a message naming the bound: no earlier than
`now - CLOCK_SKEW_MIN` (5 minutes, the same slack the session validator grants a client's own
clock) and no later than `now + MAX_LOOKAHEAD_DAYS` (14). The lower bound is what stops a
client registering a reminder in the past, where it would fire immediately and be delivered as
though it were due.

**The key is `(kind, subject_id, the user's local date)` and never the computed instant**
(A284). The server re-derives the local date from `users.timezone` rather than trusting the
client's zone, because a device whose zone is stale would otherwise register the same reminder
under a second key — which is exactly the duplicate the key exists to prevent. The write is
`insert … on conflict (dedupe_key) do nothing returning id`, so two devices that compute 19:00
and 19:01 produce **one** row and it keeps the **first** `fire_at`. `applied` says whether this
call was the one that stored it — the Phase 9 `POST /sessions` shape, for the same reason: a
client that retries after a lost response can tell whether its write landed without re-reading.
A quiet-hours deferral is applied here and not left to the sweep, so a reminder stored inside
the quiet window lands at the window's end rather than at the next tick.

`GET /reminders` is the in-app channel's feed: due, pending, and not yet acknowledged **by this
device**, newest first, `limit 20`. A `skipped` reminder stays in the feed on purpose (A286) —
a reminder that silently never arrives is what the student reports as "reminders are broken" —
and `skip_reason` is the only thing that distinguishes "quiet hours moved it past its subject"
from "the push token is dead".

`POST /reminders/:id/ack` records a `reminder_deliveries` row `(channel='web', device_id = this
device, status='sent')`, `on conflict do nothing` so a double tap is a no-op. It is **per
device**: an ack for the account would silence a reminder on a device the student had not
looked at yet.

Neither route is in the offline-write list (A283): a registration requires connectivity and
queues nothing. The client's local schedule is what covers the offline case, and it needs no
server at all.

## Dashboard (Phase 12)

`GET /dashboard`, `GET /dashboard/topics`. `POST /sessions` has moved to
`## Sessions and activity (Phase 9)`, where the timer that writes it lives.

Both routes report **only what traces to a row** (A289), which is the honesty gate's
requirement stated as a design rule: every figure is a count or a sum over rows the student
owns, and there is no composite score, no predicted grade and no "retention".

```json
GET /dashboard?from=2026-09-28&to=2026-10-12
 →  { "from", "to",
      "time_per_course": [ { "workspace_id", "title", "minutes" } ],
      "review_activity": [ { "local_date", "reviews" } ],
      "totals": { "minutes_studied", "reviews", "cards_due", "quizzes_taken" } }
```

`from`/`to` are **local days** (`YYYY-MM-DD`) in the account's zone, not instants: a session at
23:30 Jakarta is today, and a UTC window would ask for yesterday. Minutes come from
`study_sessions` joined through `workspaces`; reviews from `flashcard_reviews`; `cards_due` is
the same due-card query the study screen reads, so the number is the size of the queue a review
would actually put in front of the student. Another account's rows are never counted.

```json
GET /dashboard/topics   →  403 plan_required (min_plan "power") on a plan without
                            Feature::AdvancedAnalytics
 →  { "quiz_mastery": [ { "source_id", "title", "attempts", "correct", "accuracy" } ],
      "weak_topics":   [ …same shape… ],
      "weak_threshold": { "accuracy", "min_attempts" }, "plan_route": "/goals/{goal_id}/plan" }
```

Two routes rather than one with a flag, so the sweep can drive a real refusal and the screen
can render a locked card for the part that is locked while the rest of the dashboard keeps
working: one route would have made the whole screen a 403 on Free.

**A topic with no attempts is absent, never shown as 0%** — an uncomputable number must not
reach the screen. `weak_topics` is a filter of `quiz_mastery` rather than a second query, so
the two lists cannot disagree.

The client **renders** `weak_threshold` and does not apply it: a client-side threshold would be
a second definition of "weak", and the two would disagree the first time the server's was
tuned — the card would then show topics the server does not consider weak. Likewise the
weak-topic card names the planner as the remedy **in prose** and hands nothing over: a handover
would need a goal to attach the block to and there is no goal picker yet, so a call would have
to invent one. `plan_route` is served for Phase 11's goal picker to use; it is a planner
dependency, not a dashboard one.

## Account (Phase 15)

`DELETE /account`, `GET /account/export`. Re-tagged from Phase 7 (A160): neither
is in the roadmap's Phase 7 paragraph, and both need the retention and purge
machinery (`purge_deleted_accounts`, the `## Data retention` schedule) that ships
with the release prerequisites.

## Billing (Phase 13)

Auth, verified email for the routes that cost money, and — for the callback — no bearer
token at all.

`GET /plans`, `POST /billing/checkout`, `GET /billing/payments/:id`, `GET /billing/invoices`,
`POST /billing/upgrade`, `POST /billing/downgrade`, `POST /billing/student/verify`,
`POST /billing/student/confirm`, `POST /webhooks/qris`.

**Every price is the server's, and there is one function that computes it.** The amount a
student is quoted comes from `billing::price` over `config/plans.toml`, and the amount the
webhook checks the payment against is `payments.amount_idr` — the column the quote was
written to. Nothing on the client adds, subtracts or rounds, which is why `PaymentView`
carries `amount_before_discount_idr` and `discount_percent` as well: the screen itemises the
charge from the row rather than re-deriving it.

```json
{ "id": "uuid", "order_id": "stvdy_01j0…", "kind": "subscription",
  "plan_id": "pro", "billing_period": "monthly", "credits": null,
  "amount_idr": 10000, "amount_before_discount_idr": 10000, "discount_percent": 0,
  "discount_code": null, "student_discount": false,
  "qr_string": "…", "expires_at": "2026-10-12T07:15:00Z",
  "status": "pending", "paid_at": null }
```

- `GET /plans` → the matrix: every plan's limits and features, `credit_packs`, `currency`,
  `current_plan`, `student_verified`, and the two policy flags `show_prices` and
  `purchase_allowed`. **Prices are omitted where `show_prices` is false** (A305), which is
  what a phone gets — plan names, limits and features and no prices, including no
  credit-pack prices — because the platform is the device row (A144) and a phone must not be
  told a price it may not show. An unauthenticated caller gets 401.
- `POST /billing/checkout` body `{ "plan_id", "billing_period" }` or
  `{ "credit_pack": <credits> }`, plus an optional `"discount_code"` → **200** with a
  `PaymentView` in `pending`. **A second checkout for the same plan and period returns the
  pending invoice rather than minting a second QR** (A307): "the user paid twice for one
  period" is one of the spec's edge cases, and a second code is how it happens. The reuse key
  is every field that decides what is being bought — the user, the kind, the plan, the period
  and the credit amount. 403 `feature_locked` for `ios`/`android`
  (`config/billing_policy.toml` forbids mobile selling and the server enforces it rather than
  trusting the client — the platform is read from `devices.platform`, never the body);
  400 `validation` for a period that is not `monthly`/`yearly`, a pack that does not exist, or
  a code that cannot be used; 502 `provider_error` where no gateway is configured, decided
  **before** anything is written so no pending row is left behind; and the route requires a
  **verified email** (A43).
- `GET /billing/payments/:id` → the poll the checkout screen runs (A306), scoped by
  `user_id` in the statement rather than checked after the read. **Expiry is re-checked here**,
  so a client whose clock is wrong cannot extend its own invoice by not noticing: a `pending`
  row past its expiry reads `expired`. 404 for another account's payment.
- `GET /billing/invoices` → `{ "invoices": [PaymentView…] }`, the caller's own newest 50.
  It does **not** re-check expiry, unlike the single-payment read: this is a read of what
  happened, and a list that quietly rewrote rows while the student was looking at their
  history would be a surprise.
- `POST /billing/upgrade` / `POST /billing/downgrade` body
  `{ "plan_id", "billing_period" }`. An **upgrade** prices the prorated difference for the
  remaining days and mints an invoice for it; the plan does not move until that invoice is
  paid, because until the money arrives the student is on the plan they paid for — and moving
  it now would make the payment look like a renewal and hand them a free period. A
  **downgrade** is recorded, not applied: it writes the pending columns and nothing else, and
  the rollover applies it when the period ends, so a downgrade never shortens time already
  paid for. Both refuse a mobile platform with 403 `feature_locked`, and both need a verified
  email.
- `POST /billing/student/verify` body `{ "email" }` → mails a link to that address, rate
  limited and only for a suffix in `STUDENT_EMAIL_SUFFIXES` (default `.ac.id`), matched on the
  **domain** so `user@ui.ac.id` matches and `user@ac.id.evil.example` does not. Without both
  the volume rule and the content rule the endpoint is a mail amplifier pointed at a campus
  domain. `POST /billing/student/confirm` body `{ "token" }` sets `student_verified` and
  `student_email` **from the address the link was mailed to** — there is no body field for it,
  because that is what proves control of a campus address. The discount then applies at
  checkout from `plans.<id>.student_percent_off`, and the student promo and a discount code
  **do not stack**: the larger percentage wins and a tie goes to the student promo, which needs
  no redemption.
- `POST /webhooks/qris` — the gateway's callback, and the only route in the API with **no
  bearer token**. Its authorisation is the signature over the **raw bytes** in
  `QRIS_SIGNATURE_HEADER`, one of three schemes (`hmac_sha256_hex`, `hmac_sha256_base64`,
  `shared_token`), and the order of operations is the whole security story: the IP allowlist
  (when `QRIS_WEBHOOK_IPS` is set), then the signature, then the parse, then the idempotency
  insert, then the guarded transition and the apply. A body that fails the signature is never
  parsed and a request that fails any step writes nothing — an unknown `order_id` is 404 with
  the `webhook_events` row rolled back, so an unauthenticated caller cannot write rows with a
  guessed order id. **401** for a bad signature, **404** for an order we do not have, **502**
  where no gateway is configured (so the gateway retries rather than reading a valid callback
  as forged), and **200** with `{ "duplicate": bool, "status": string }` otherwise.

  **Idempotency has two defences because there are two replays** (A293). The *same* event id
  twice is caught by `webhook_events`' `unique (provider, event_id)` with
  `on conflict do nothing returning id`: no row back means already handled, and the answer is
  `{ "duplicate": true, "status": "duplicate" }`. The *same payment* under a **new** event id —
  which is what a real gateway does when it re-sends — is caught by the transition itself
  being `update payments set status = 'paid' … where id = $1 and status = 'pending'`, so a
  second report updates zero rows and the answer is `"already_paid"`. A defence against one
  does nothing against the other.

  **The amount is checked after the transition, deliberately**: a mismatch is marked
  paid-but-under-review rather than left `pending` for a retry loop, and both directions (over-
  and under-payment) are the same outcome, `"review"`, with the two numbers written to
  `review_reason` for the operator who reads the queue. A non-IDR currency is refused the same
  way. A payment made **after** the invoice expired is `review` unconditionally; a payment made
  before the expiry whose *confirmation* arrived late is honoured when `QRIS_HONOR_LATE_PAYMENT`
  (default true) is set, because the lateness is ours and must not cost the student their plan.
  The money moves in **one transaction**: mark paid, extend or start the period, grant the
  credits, record the fee, notify — all of it, or none of it.

