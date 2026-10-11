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
  Fields: `kind` in `pdf` or `text`; then `file` (a `.pdf`) for `pdf`, or `text`
  for `text`; optional `title` (defaults to the filename, or to the first
  non-empty line of pasted text). The source is stored `pending` and an
  `ingest_source` job is enqueued. 400 `validation` for a missing or unsupported
  `kind`, a missing file or text, a non-`.pdf` filename, or empty text. 404 for
  another user's workspace. 422 `limit_reached` for `max_file_mb` or `storage_mb`.
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

`POST /workspaces/:id/reports/outline`, `POST /reports/:id/generate`,
`PATCH /reports/:id`, `GET /reports/:id/export?format=`.

## Cheatsheets (Phase 6)

`POST /workspaces/:id/cheatsheets` (body: scope, exam_date, depth, source_ids),
`GET /cheatsheets/:id`, `GET /cheatsheets/:id/html`, `GET /cheatsheets/:id/pdf`,
`POST /cheatsheets/:id/re-render`.

`re-render` regenerates HTML and PDF from the stored JSON blocks without calling
the model, so a template fix can reach documents users already have.

## Tasks (Phase 9)

CRUD `/tasks`, `POST /tasks/quick-capture`, `POST /tasks/:id/breakdown`.

## Events (Phase 9)

CRUD `/events`, `GET /events?from=&to=`.

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

## Study and dashboard (Phase 12)

`POST /sessions`, `GET /dashboard`.

## Settings (Phase 9)

`GET /settings`, `PUT /settings`, `PUT /settings/api-key`,
`DELETE /settings/api-key`.

## Account (Phase 7)

`DELETE /account`, `GET /account/export`.

## Billing (Phase 13)

`GET /plans`, `GET /billing/me`, `POST /billing/checkout`,
`GET /billing/payments/:id`, `GET /billing/invoices`,
`POST /webhooks/qris` (signature-verified, not bearer-authenticated).

`POST /billing/checkout` is rejected for `platform in (ios, android)`:
`config/billing_policy.toml` forbids mobile selling, and the server enforces it
rather than trusting the client.

## Admin (Phase 13)

Users, plan overrides, manual payment mark/refund, credit grants, margin report.
