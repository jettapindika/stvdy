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
  409 conflict, 410 `resync_required`, 422 limit reached (`limit_reached`),
  429 rate limited, 502 provider error.
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
  the client's point of view. 403 for a bad or expired signature; 404 on the S3
  backend, where this route is not part of the contract.

## Chat (Phase 2, implemented)

### `POST /workspaces/:id/chat`

Auth (and the email must be verified: chat is one of the four actions A43 keeps
locked until then, so an unverified account gets 403 `email_not_verified`).
Answers `text/event-stream`.

Body: `{"message": string, "source_ids"?: [uuid], "use_web"?: bool,
"study_mode"?: bool}`. `source_ids` narrows retrieval to the selected sources; an
absent or empty list searches the whole workspace. `use_web` is recorded on the
message; the search arm itself arrives in Phase 3.

Order of operations, which is the contract rather than an implementation detail:
verified email, owned workspace, daily message cap, credit spend, persist the user
message, retrieve, compose, stream. Every check that can fail therefore fails
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
citations" from "the stream died". `error` can also arrive **mid-stream**, after
tokens, if the assistant message fails to persist: a client must handle an `error`
at any position. A `: heartbeat` comment line is sent every 15 seconds to keep a
proxy from closing an idle stream, and a client's parser must ignore comment
lines rather than read them as events.

Citation labels are numeric (`[1]`, `[2]`) and resolve against the context the
server sent. A label the model invents is dropped from both the citations and the
stored text (A17), so a reply can never show a citation the user cannot open.

The assistant message is persisted **after** the stream completes, with the
resolved citations. A client that disconnects mid-answer therefore leaves the
user's turn and no half-answer.

### `GET /workspaces/:id/messages`

Auth. 200 `{"messages":[{"id","workspace_id","role","content","citations",
"used_web","created_at"}]}`, oldest first, where `role` is `user` or `assistant`
and `citations` is the same array shape as the event. This is what makes a thread
visible on the account's other devices. Another account's workspace is a 404.

## Flashcards (Phase 4)

`POST /workspaces/:id/decks`, `GET /decks/:id`, `GET /decks/:id/due`,
`POST /flashcards/:id/review`.

## Quiz (Phase 5)

`POST /workspaces/:id/quizzes`, `GET /quizzes/:id`,
`POST /quizzes/:id/attempts`, `GET /quizzes/:id/results`.

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
