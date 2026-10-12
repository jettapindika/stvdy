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
```

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

`GET /sync/pull?cursor=&limit=`, `POST /sync/push`, `GET /sync/stream`.

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

CRUD `/goals`, `POST /goals/:id/plan`, `POST /plans/accept`, `POST /plan/replan`.

## Syllabus (Phase 11)

`POST /workspaces/:id/syllabus`, `GET /syllabus/:id/draft`,
`POST /syllabus/:id/confirm`.

## Calendar (Phase 10)

`GET /calendar/google/connect`, `GET /calendar/google/callback`,
`POST /calendar/ics`, `DELETE /calendar/connections/:id`,
`POST /calendar/sync`, `POST /calendar/ics-token/regenerate`,
`GET /cal/:token.ics` (public, token-authenticated).

## Actions (Phase 12)

`GET /actions/pending`, `POST /actions/:id/confirm`, `POST /actions/:id/reject`.

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
  "timezone": "Asia/Jakarta" }
```

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

## Dashboard (Phase 12)

`GET /dashboard`. `POST /sessions` has moved to `## Sessions and activity (Phase
9)`, where the timer that writes it lives.

## Account (Phase 15)

`DELETE /account`, `GET /account/export`. Re-tagged from Phase 7 (A160): neither
is in the roadmap's Phase 7 paragraph, and both need the retention and purge
machinery (`purge_deleted_accounts`, the `## Data retention` schedule) that ships
with the release prerequisites.

## Billing (Phase 13)

`GET /plans`, `GET /billing/me`, `POST /billing/checkout`,
`GET /billing/payments/:id`, `GET /billing/invoices`,
`POST /webhooks/qris` (signature-verified, not bearer-authenticated).

`POST /billing/checkout` is rejected for `platform in (ios, android)`:
`config/billing_policy.toml` forbids mobile selling, and the server enforces it
rather than trusting the client.

## Admin (Phase 13)

Users, plan overrides, manual payment mark/refund, credit grants, margin report.
The plan override, the credit grant and the margin report are already live from
Phase 7 (`## Admin (Phase 7)` above); this phase adds the payment half and the
completed margin report (revenue and conversion, which Phase 7 returns as
`null`).
