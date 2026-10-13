# The workspace behind a verified address (A359)

## What changes

A43 was "a verified email before any action that **costs money**". A359 changes the axis to the
**surface**: signup still creates the account, and nothing else in the app works until the
address is confirmed. An unverified account keeps exactly its own administrative surface —
session, devices, settings, export, deletion — and is refused everything else with `403
email_not_verified`.

## Why the old boundary was not enough

The old rule gated the routes that spend provider money: chat, deck/quiz/report generation,
search, ingestion, checkout. It left `POST /workspaces` on `AuthUser` deliberately, with the
comment "creating a workspace spends nothing and costs no provider call, so A43 does not
apply".

The argument is sound about money and wrong about abuse. An unverified account can hold a
workspace, and the read routes around it (`GET /workspaces/{id}`, `/messages`, `/decks`,
`/sources`, `/usage`, the whole sync pull) are not gated by A43 either. So a scripted account
gets a durable, readable container with no address behind it — at no cost to the script and
some cost to the operator, which is the free-tier abuse A43 exists to bound (A43, A46). The
price of an action was never the right proxy for whether it belongs to a proven owner.

## The boundary

| Reachable unverified | Refused unverified |
|---|---|
| `GET /auth/me`, `POST /auth/logout-all` | every workspace route |
| `POST /auth/resend-verification` | chat, decks, quizzes, reports, cheatsheets, study |
| `POST /auth/google/set-password` | tasks, events, calendar, planner, goals, actions |
| `GET`/`PATCH`/`DELETE /devices*`, `PUT /devices/{id}/push-token` | sources, chunks, syllabus, reminders, dashboard |
| `GET`/`PUT /settings`, `…/api-key` | sync (pull, push, stream) |
| `GET /usage` | billing: checkout, upgrade, downgrade |
| `DELETE /account`, `GET /account/export` | |
| `GET /plans`, the payment poll, the invoice list, the student-discount pair | |

**The open column is not a convenience, it is the exit.** A student who typed an address they
cannot receive mail at has to be able to revoke a device, take their data out (`GET
/account/export`) and delete the account. Refusing those is refusing the one remedy the store
rules and `docs/EULA.md` § 9 promise, and it turns a gate into a trap. `GET /usage` and the
billing reads are open for the same reason a plan limit never hides the caller's own numbers
(A149) — and because checkout is what needs a verified address, not the price list.

## Enforcement, and why it is in two places

**The server is the enforcement** (A42: the client is attacker-controlled). Every refused route
takes `VerifiedUser` instead of `AuthUser`; `VerifiedUser` derefs to `AuthUser`, so handler
bodies and helpers are unchanged. 80 handlers moved.

**The client is the explanation.** The shell holds an unverified session at `/verify` for every
route outside `/settings`. Rendering the shell would be twenty screens of `403
email_not_verified`, which names the fact and not the remedy. The gate is one `if` in `App`,
where the session gate already is, because a per-screen check is a check some screen forgets.

**The route sweep is the guarantee that neither drifts.** `route_sweep.rs` gains a `verified`
column on every row and two assertions:

- `every_route_takes_the_extractor_the_table_names` reads each `.route(...)` call, resolves the
  handler to the file that declares it, and compares its `user:` parameter's type against the
  row. A new route has to choose a side, and a route whose handler disagrees with its row fails.
  This is the assertion the route table's own comment claimed existed for several phases
  ("`route_sweep.rs` asserts that statically") while nothing did — which is precisely how the
  workspace stayed open.
- `an_unverified_account_reaches_only_its_own_surface` drives all 85 routes with an unverified
  token **in both directions**: a gated route must answer `403 email_not_verified`, and an open
  route must not. The second direction is the one that catches the trap.

`DELETE /account` and `POST /auth/logout-all` are driven last: the first marks the account and
the second revokes the token every later row needs.

## The client surface

- `App` redirects an unverified session to `/verify` with `replace`, except under `/settings`.
- `SignInScreen` and the Google callback land on `/verify` rather than bouncing off `/workspaces`.
- `VerifyEmailScreen`'s awaiting-link view loses its "Go to your workspaces" button — there is no
  workspace to go to — and gains **"Use a different address"**, a sign-out. Without it a mistyped
  address is permanent.
- The verified view waits for `/auth/me` to report the new state before offering the workspace
  link: until it does, the gate would bounce the student back and re-spend the single-use token.
- `useSignOut` is new: `POST /auth/logout` with the stored refresh token, then `clearSession`.
  It clears the store even when the request fails — a sign-out that left a working token because
  the network was down would be a lie about the one action whose purpose is to leave.

## Costs and what was rejected

- **Every existing test that signs up and then calls a workspace route** goes through
  `common::signed_up`, which verifies the address. Only the tests that are *about* the
  unverified state had to change: `abuse.rs` (its workspace assertion inverts) and the sweep.
- **A legacy unverified account** loses read access to data it already has. It keeps export and
  deletion, so nothing is held hostage; the alternative — reads stay open — is the hole itself.
- **Rejected: a middleware layer** instead of the extractor. A `route_layer` deciding from
  `MatchedPath` would need a second list of "the routes that need verification", and two lists
  drift (the same argument `the_spend_cap_covers_only_swept_routes` makes). The extractor is
  the type system's version: a handler that takes `VerifiedUser` cannot be called unverified.
- **Rejected: gating on `email_verified` inside each handler.** A line a later edit can drop is
  not a guarantee; that is why `checkout.rs` already used the extractor for its half of A43.
