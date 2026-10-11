# API

Base URL is the server origin. Every route below requires
`Authorization: Bearer <access_token>` unless marked public. Bodies and responses
are JSON unless noted.

## Conventions

- **Errors** are always `{"code": "...", "message": "...", "min_plan": null}`.
  `code` is machine-readable and stable; `message` is for logs, not for showing
  to users. Entitlement errors add `min_plan`.
- **Status codes**: 401 unauthorized/expired token, 402 insufficient credits,
  403 plan required or feature locked, 404 not found, 409 conflict,
  410 `resync_required`, 422 limit reached, 429 rate limited, 502 provider error.
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

`POST /auth/signup`, `POST /auth/login`, `POST /auth/refresh`,
`POST /auth/logout`, `POST /auth/logout-all`, `POST /auth/verify-email`.

## Devices (Phase 1)

`GET /devices`, `PATCH /devices/:id`, `DELETE /devices/:id`,
`PUT /devices/:id/push-token`.

## Sync (Phase 8)

`GET /sync/pull?cursor=&limit=`, `POST /sync/push`, `GET /sync/stream`.

## Workspaces (Phase 1)

CRUD `/workspaces`.

## Sources (Phase 1)

`POST /workspaces/:id/sources`, list, `DELETE`, `GET /sources/:id/status`,
`GET /sources/:id/file`, `GET /chunks/:id`.

## Chat (Phase 2)

`GET /workspaces/:id/messages`, `POST /workspaces/:id/chat` (SSE).

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
