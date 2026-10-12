# Operations

The day-two runbook. `docs/RELEASE.md` is the normative release document — what is
distributed, under what policy, with which pinned components; this file is how an operator
runs and checks the server that ships from it.

Written by Phase 15's image work (Tasks 6–9) and its operations work (Task 13). The sections
below § Health — the production host checklist, backups and the restore drill, monitoring,
the breach runbook and retention — are the day-two procedures, and each is written as steps
rather than as a description, because a runbook nobody could follow at 03:00 is not a runbook.

**Three things in this file are launch prerequisites rather than reference material**, and the
launch checklist in `docs/RELEASE.md` names them by row: the restore drill must have been run
and its output recorded (§ Backups and the restore drill), the uptime check and alert routing
must be live (§ Monitoring and alerting), and the breach runbook's two blanks must be filled
in (§ The breach runbook). Two facts a runbook needs are **stated as blanks** rather than
invented: who decides, and how long the notification window is. Inventing either would be
worse than leaving it visible.

## The server image

One image runs the whole server: API, job worker and scheduler, as one process
(`crates/server/src/main.rs`). It carries three native dependencies the code loads at
runtime, each pinned in the `Dockerfile` and each with an absent-binary policy — because the
failure of getting one wrong is a feature that quietly stops working rather than a failed
start (`docs/RELEASE.md`).

| Component | Version | Env | Absent |
|---|---|---|---|
| Chromium | `ARG CHROMIUM_VERSION` (Debian bookworm) | `CHROME_PATH=/usr/bin/chromium` | server starts; `GET /cheatsheets/:id/pdf` answers 409, a report's `format=pdf` answers 502 |
| pdfium | `chromium/8086`, checksum-verified | `PDFIUM_LIB_PATH=/opt/pdfium/lib/libpdfium.so` | PDF ingestion is unavailable and the job fails with a message naming the variable (A51) |
| Embedding model | `intfloat/multilingual-e5-base`, pre-baked | `FASTEMBED_CACHE_DIR=/opt/models/fastembed` | the first ingest downloads 1.06 GB, which on a host with no egress is a stall or a failure (A2) |
| LibreOffice | bookworm's `libreoffice-core`/`-writer`/`-impress` | `SOFFICE_BIN=/usr/bin/soffice` | the source still ingests, the preview is absent, and `GET /sources/:id/preview` answers 404 (A158) |
| Print assets | KaTeX 0.19.0 + Inter/JetBrains Mono | `ASSETS_DIR=/opt/assets` | **not** a runtime dependency: `rust-embed` compiled them into the binary and `GET /assets/{*path}` serves that copy (A339) |

The image runs as the non-root user `stvdy` (uid 10001) with Chromium's sandbox **on**. The
renderer refuses an unsandboxed render outside dev, so `--no-sandbox` is not the fix for a
sandbox failure: if Chromium will not start here, that is a finding to report, not a flag to
add (A338).

The sandbox needs two things the container runtime must provide:

```sh
docker run -d --name stvdy \
  --shm-size=1g --tmpfs /tmp \
  -e DATABASE_URL=… -e APP_ENV=prod \
  -e JWT_SECRET=… -e ENCRYPTION_KEY_HEX=… -e WEB_ORIGIN=https://… \
  -e STORAGE_BACKEND=s3 -e S3_ENDPOINT=… -e S3_BUCKET=… \
  -p 8080:8080 \
  stvdy-server:<tag>
```

`--shm-size` and a writable `/tmp` are both required. A missing `/dev/shm` is the classic
"Chromium dies with a memory error that looks like a code bug" failure.

## Deploying a new image

**The migration runs to completion before any replica serves traffic.** That is a deployment
contract, not a convenience:

> Two replicas starting together both run `sqlx::migrate!` against one database. The advisory
> lock makes that survivable, but the replica that loses the race then serves traffic against
> a schema it has not verified. `--migrate-only` is what lets the orchestrator sequence the
> step, and it exits without binding a port — so "it exited 0" and "it is serving" are
> different facts, and the deploy procedure depends on the difference (A346).

```sh
# 1. Migrate. Runs to completion and exits. No port is bound.
docker run --rm --network host \
  -e DATABASE_URL="$DATABASE_URL" \
  stvdy-server:<tag> --migrate-only

# 2. Only then start the replica(s).
docker run -d … stvdy-server:<tag>
```

The step also reconciles the `plans` table with `config/plans.toml` (`seed_plans`), so the
replicas do not race on that either. A missed or misspelled flag is refused with
`unknown argument …` rather than ignored, because a typo that silently started the server
would run the migration from every replica — the exact race the flag removes.

`--migrate-only` does **not** change which migration is last: `0017_sync_triggers.sql` stays
the last trigger-attaching migration, and `crates/server/tests/release.rs` asserts the sync
triggers exist after the step runs.

## Before deploying: run the image check on the host

"Works in CI" and "works on this host" are different claims, and the second is the one that
ships. The same sequence CI runs is a script, and it is meant to be run **on the production
host** against the image about to be deployed:

```sh
IMAGE=stvdy-server:<tag> DATABASE_URL="$DATABASE_URL" bash scripts/release-image.sh
```

It builds nothing it does not have to, then checks, in order:

1. the version the image's Chromium reports equals the `ARG CHROMIUM_VERSION` in the
   `Dockerfile` — an image that silently tracked Debian's `stable` passes every other check
   here and changes every PDF;
2. pdfium, LibreOffice and a **non-empty** model cache exist at the paths the environment
   names;
3. `--migrate-only` succeeds;
4. the server boots and `/health` answers `"status":"ok"`;
5. `GET /assets/{katex.min.css,katex.min.js,contrib/auto-render.min.js,fonts.css}` answer 200;
6. the PDF acceptance test passes **inside the container**, against the image's own browser.

It ends with `=== PASS`. Step 6 is the one that matters: `crates/server/tests/pdf.rs` is
`#[ignore]`d so a laptop without a browser still gets a green `cargo test`, and it asserts the
rendered PDF is six A4 pages whose extracted text contains no raw `$`. A pass therefore proves
the print assets resolved, because a missing KaTeX leaves every formula as literal LaTeX — the
silent defect the assertion exists for.

## Environment variables

Beyond the application's own configuration (see `.env.example`), the image sets or expects:

| Variable | Default in the image | Notes |
|---|---|---|
| `APP_ENV` | `prod` | prod requires `JWT_SECRET` (≥32 chars) and `WEB_ORIGIN` |
| `BIND_ADDR` | `0.0.0.0:8080` | the port here is what `ASSETS_BASE_URL` defaults to |
| `CHROME_PATH` | `/usr/bin/chromium` | read first by `find_executable` |
| `PDFIUM_LIB_PATH` | `/opt/pdfium/lib/libpdfium.so` | a set-but-unloadable path is a **boot** failure (A51) |
| `FASTEMBED_CACHE_DIR` | `/opt/models/fastembed` | absolute on purpose: the config default is relative, which in a container means "relative to the working directory" (A342) |
| `SOFFICE_BIN` | `/usr/bin/soffice` | dormant until Phase 7's PPTX/DOCX ingestion lands (A343) |
| `ASSETS_DIR` | `/opt/assets` | the operator's copy of the vendored print assets, for inspection; the served bytes are the embedded ones |
| `ASSETS_BASE_URL` | `http://127.0.0.1:<BIND_ADDR port>` | the origin the renderer rewrites the template's asset references to. The default is what makes a single container work with no configuration; set it when Chromium cannot reach the server's advertised address, or when the renderer runs beside the server rather than inside it (A339) |

`FASTEMBED_CACHE_DIR` is the one to check after a deployment that changed `WORKDIR`: the
config's default is relative, and a cache that moved is a 1.06 GB download at the first
ingest rather than an error at boot.

## Boot sequence

In order, and each is a failure rather than a warning unless stated:

1. configuration loads and validates — a broken `plans.toml` is a startup failure, not a 500
   on the first request;
2. Postgres connects, migrations run, plans are seeded;
3. provider assertions: the storage backend is reachable, the bucket exists, pdfium binds,
   the embedding dimension agrees with the `chunks.embedding` column;
4. jobs left `running` by a killed process are requeued, and refunds left pending by a failed
   generation are recovered;
5. the worker and the scheduler start;
6. the port binds and `/health` reports `ok` with the version from `CARGO_PKG_VERSION`.

Step 3 is where a missing native dependency shows up. It is deliberately at boot rather than
at first use: a configured extractor that silently does nothing is worse than an absent one,
which is why an unset `PDFIUM_LIB_PATH` logs "PDF ingestion is disabled" while a set-but-broken
one fails the start.

## Health

`GET /health` is liveness plus a cheap readiness probe: the pool answers and the plans config
loaded. It reports `status`, the server version (`CARGO_PKG_VERSION` — the server is versioned
independently of the desktop shell, A353) and `env`.

It does **not** report whether Chromium launched. A deployment with no browser is healthy and
serves everything except PDFs, which is why the image check above probes the PDF path directly
rather than trusting `/health` (A130).

---

## The production host

Read this before buying anything. Every row has a **pass condition**, so "is this host suitable"
has an answer rather than an opinion (A330). A host that fails any row is not the production host,
whatever else it offers.

| # | Requirement | Pass condition | Why |
|---|---|---|---|
| 1 | **An Indonesian region** | the API, the database, the bucket and SearXNG are all in an Indonesian region, and the vendor documents it | UU PDP's data-residency expectation, and the latency the Batam trial demonstrated. One region for all four, not four regions that agree by accident |
| 2 | **Managed Postgres 16 with pgvector** | `select extversion from pg_extension where extname='vector'` returns a version, and the migrations run to completion | The migrations create the extensions and the `vector` columns. A container volume holding the database is *not* this row: it has no PITR, and A347's RPO of 15 minutes is unreachable without it |
| 3 | **A lifetime that matches the product** | a paid term of at least a year, not a trial | **A45's trial host (`168.110.202.106`) is disqualified.** A 1-month trial that the QRIS webhook or the update manifest points at is an outage with a date on it |
| 4 | **Egress** | outbound HTTPS to the model provider, the search tier if it is not local, SMTP and Apple's notary service | The build stage needs it (the embedding model, the pdfium download), and so does a live ingest. A host with no egress turns the first ingest into a 1.06 GB stall |
| 5 | **A static IP or a DNS name you control** | a name that resolves and that you can point at the host without a vendor's help | The QRIS webhook URL and the update manifest endpoint are both published values. A host whose address changes is a webhook that stops arriving and an updater that reports "up to date" forever |
| 6 | **One container with `--shm-size` and a writable `/tmp`** | `docker run` accepts both flags | Chromium's sandbox needs `/dev/shm` and a writable `/tmp`. Without them the browser dies with a memory error that reads like a code bug (A338) |
| 7 | **TLS termination** | a certificate that renews without a human, and `WEB_ORIGIN` matching the web build's origin | The app's CORS is origin-checked and the plugin refuses a non-HTTPS update endpoint. A self-signed certificate is not this row |
| 8 | **Enough disk for the model cache and the object cache** | at least 5 GB free after the image | The image carries 1.06 GB of embedding weights plus the release `target/` directory (A337, A342) |
| 9 | **A place for the dumps** | either the host's own PITR, or a volume that `pg_dump` writes to and R2 receives | A347's restore drill reads the newest dump. A host where the dump has nowhere to land is a host with no backup |

**What is *not* on this list, deliberately:** a load balancer, a second replica, or an autoscaling
group. `crates/server/src/main.rs` runs the API, the worker and the scheduler as three tasks in one
process, so one container is the whole server and a second replica is a later concern that this
release does not need. Adding one before it is needed is how a deployment acquires a race it did
not have — the migration race `--migrate-only` exists to remove (A346).

## The reverse proxy and the public surface

Three rules, and each is a defect if it is missing rather than a tuning choice.

1. **TLS terminates here**, and everything below it is plain HTTP on a private network.
2. **Buffering off on the SSE route** (`GET /chat/…/stream` and the sync stream). A proxy that
   buffers holds the response until it is complete, which for a stream is never — the client sees
   nothing and the connection eventually dies. **The heartbeat exists precisely because proxies
   close idle streams**, so disabling buffering is what makes the heartbeat sufficient rather than
   the heartbeat being what makes buffering acceptable.
3. **`GET /assets/` is public and cacheable.** No auth, no rate limit, no `Set-Cookie`. The
   requester is headless Chromium printing a document with no credentials, and every file is a
   vendored upstream release with no user data in it (A339). The route answers
   `Cache-Control: public, max-age=31536000, immutable` itself; the proxy should pass that through
   rather than shortening it.

## Backups and the restore drill (A347)

**RPO 15 minutes, RTO 4 hours.** Both are stated because "we have backups" is not a claim a
restore can fail, and because the two numbers decide what is acceptable: 15 minutes bounds how much
a student's work can be lost, and 4 hours bounds how long the product can be down.

### Postgres

**Preferred: the host's own PITR.** Managed point-in-time recovery to within 15 minutes is what
makes the RPO reachable; a nightly logical dump does not. Confirm two things on the host, and
record the answers in this section with a date:

1. the vendor's documented recovery window covers 15 minutes (not "daily backups");
2. a restore to a point in time has been *attempted* — by the vendor's own console if it offers
   one, and by this drill otherwise.

**Otherwise: a `pg_dump` on a timer to R2.**

```sh
# /etc/cron.d/stvdy-backup — every 15 minutes is what the RPO asks for; hourly is the
# least a deployment should accept, and the RPO in this document then needs correcting
# rather than quietly not being met.
*/15 * * * * root pg_dump -Fc "$DATABASE_URL" > /var/backups/stvdy/stvdy-$(date +\%Y\%m\%dT\%H\%M).dump
*/15 * * * * root /usr/local/bin/rclone copy /var/backups/stvdy r2:stvdy-backups/db
5 4 * * *    root find /var/backups/stvdy -name '*.dump' -mtime +30 -delete
15 4 * * *   root bash /opt/stvdy/scripts/restore-drill.sh --dump /var/backups/stvdy --target-url "$DRILL_DATABASE_URL" >> /var/log/stvdy-drill.log 2>&1
```

The 04:15 line is the one that matters: **a daily verification that the newest dump is non-empty,
restores, and holds the data** — not merely that the cron job exited 0. A dump that is 0 bytes
restores to a valid, empty database, and "it restored" is true of that. The drill catches it
because it asserts the schema and the row counts, not the exit code.

30 days of retention is what the RPO and RTO imply rather than an independent decision: a bug
noticed a month later is still recoverable, and the bucket stays small.

### R2

- **Bucket versioning on.** A presigned URL handed to a client cannot delete anything, but an
  application bug or a bad deploy can, and versioning is what makes that recoverable.
- **A lifecycle rule** on the `reports/` and `cheatsheets/` prefixes, so generated documents expire
  on a stated schedule rather than accumulating. See § Retention for the windows.
- The bucket is **not public** (A331), so a restore does not have to worry about what was exposed
  while it was being recovered.

### The drill, and where its output goes

```sh
bash scripts/restore-drill.sh \
  --dump /var/backups/stvdy \
  --target-url postgres://…/stvdy_drill \
  --source-url "$DATABASE_URL"
```

It restores the newest dump into a scratch database, requires `sqlx migrate info` to report every
migration installed and none pending or missing, and compares the sixteen `+sync` tables' row
counts against the live database. It ends with `=== PASS`, and it exits 2 rather than 1 when it
*could not run at all* — because "the restore is bad" and "the drill did not happen" call for
different responses.

**Run it before launch and quarterly after, and paste the output here with a date.** The tolerance
is the honest part: production keeps writing during a restore, so exact equality is a claim a busy
system cannot make. The default is 0 because the pre-launch run is against a quiet database and
equality is the stronger claim; a deployment that cannot stop writes passes `--tolerance` and
records the number it passed.

**The production row is still blank, and it is blank on purpose.** The run below is real output
from a real Postgres 17 cluster, but it is a **local** one: the production host is row 12 of the
launch checklist and nobody has provisioned it. A drill against production is what the checklist
asks for, and typing a plausible result here from memory would be worse than leaving it empty.

```
2026-10-12 — the first recorded run. `pg_dump -Fc` of a freshly migrated database, seeded with
one user, one workspace and one task, restored into a scratch database on the same cluster:

$ scripts/restore-drill.sh --dump /tmp/stvdy-backups \
    --target-url postgres://…/stvdy_drill --source-url postgres://…/stvdy
newest dump: /tmp/stvdy-backups/stvdy-20261012.dump (2026-10-12 11:24:58)
format:     pg_restore (142997 bytes)

=== restoring /tmp/stvdy-backups/stvdy-20261012.dump into stvdy_drill

=== schema
migrations: 34 installed, 0 pending, 0 missing (expected 34)

=== row counts, the +sync tables
table                    restored       source
workspaces                      1            1
sources                         0            0
messages                        0            0
decks                           0            0
flashcards                      0            0
flashcard_reviews               0            0
quizzes                         0            0
quiz_questions                  0            0
quiz_attempts                   0            0
reports                         0            0
cheatsheets                     0            0
tasks                           1            1
goals                           0            0
events                          0            0
study_sessions                  0            0
pending_actions                 0            0

=== PASS
restored stvdy-20261012.dump (142997 bytes) into stvdy_drill:
  34 migrations installed, none pending or missing
  2 rows across the 16 +sync tables
```

The failure paths were exercised in the same session, because a drill that has only ever passed is
a drill nobody has seen fail: a 0-byte dump (exit 1), an empty backup directory (exit 1), a
corrupt/truncated dump (exit 1), a `--schema-only` dump (exit 1 — "0 installed, 34 pending"), a
source ahead of the dump (exit 1, naming the table and the delta), the same case with
`--tolerance 5` (exit 0), and `--source-url` equal to `--target-url` (exit 2, refused before
anything was restored). The plain-SQL and gzipped formats were run too, both passing.

**Re-run 2026-10-12 against the development database, which carries real data rather than one
workspace.** The shape is identical and the numbers are the interesting part: the migration count
has moved from 34 to **36**, and the row counts are non-trivial, so this run exercises the
comparison rather than comparing sixteen zeroes. A drill whose only recorded output is a database
with two rows in it has not exercised the row-count check at all.

```
$ pg_dump -Fc "$DATABASE_URL" > /tmp/drill-dumps/stvdy-20261012T1843.dump
$ scripts/restore-drill.sh --dump /tmp/drill-dumps \
    --target-url postgres://…/stvdy_drill --source-url postgres://…/stvdy

=== restoring /tmp/drill-dumps/stvdy-20261012T1843.dump into stvdy_drill

=== schema
migrations: 36 installed, 0 pending, 0 missing (expected 36)

=== row counts, the +sync tables
table                    restored       source
workspaces                    107          107
sources                        96           96
messages                      170          170
decks                           1            1
flashcards                     10           10
flashcard_reviews               1            1
quizzes                         4            4
quiz_questions                  8            8
quiz_attempts                  18           18
reports                        12           12
cheatsheets                     9            9
tasks                          79           79
goals                          27           27
events                        258          258
study_sessions                  6            6
pending_actions                 6            6

=== PASS
restored stvdy-20261012T1843.dump (1088100 bytes) into stvdy_drill:
  36 migrations installed, none pending or missing
  812 rows across the 16 +sync tables
```

**Replace these blocks with the production run, dated, before launch** (launch checklist row 14).
What the local runs establish is that the drill works and that it tracks the schema; what they
cannot establish is anything about the production host, the R2 bucket, or the vendor's recovery
window.

## Monitoring and alerting (A348)

Four signals, each with a threshold and an owner. **The alerting source is external** for the
first one on purpose: a server that is down cannot report that it is down.

| # | Signal | How | Threshold | Why this one |
|---|---|---|---|---|
| 1 | **Liveness** | an external uptime check against `GET /health`, 60 s interval | alert after **two consecutive failures** | `/health` touches the pool and the plans config, so it is a real readiness probe rather than a process check. Two failures rather than one, so a single dropped packet is not a page |
| 2 | **Job backlog age** | `select min(created_at) from jobs where status = 'queued'` on a timer | alert above **15 minutes** | A stuck worker is invisible otherwise: a queue that is not draining looks exactly like a queue with nothing in it, and the symptom a student reports is "my PDF never appeared" |
| 3 | **Provider spend** | the month's `sum(provider_cost_usd)` per plan against `provider_cost_ceiling_usd` | alert at **80%** | The ceiling is enforced in code (A41), so the *only* thing missing is being told when it trips — at 100% the free tier stops working for everyone and nobody knows why |
| 4 | **5xx rate** | the reverse proxy's log, or the server's own counters | alert above a **5% 5xx rate over five minutes** | The catch-all. Signals 1-3 are specific and this one is what catches the failure nobody thought to enumerate |

```sh
# Signals 2 and 3, as one query each, for whatever runs the check.
psql "$DATABASE_URL" -tAc \
  "select coalesce(extract(epoch from now() - min(created_at))::int, 0) from jobs where status = 'queued'"
psql "$DATABASE_URL" -tAc \
  "select coalesce(max(round(100.0 * spend / ceiling)), 0) from (…)"   # see crates/server/src/admin.rs
```

**Alert routing is the operator's, and this section does not invent one.** Two destinations are
needed — an address a human reads, and a channel that survives a mail outage:

| Destination | Value |
|---|---|
| Where alerts go | `<the mail address or chat webhook the operator watches>` |
| Who is on the other end | `<a name, not a role: "the on-call" is nobody at 03:00>` |
| What is *not* alerted on, deliberately | a single failed request, a 4xx rate, or disk above 80%. Each is a finding to look at during the day, not a page — an alert nobody acts on is an alert everybody learns to ignore |

**One signal cannot be a threshold, and it is stated here rather than omitted.** A missing
Chromium in the image is *not* a health failure: the server is healthy and serves everything except
PDFs (`GET /cheatsheets/:id/pdf` answers 409 `conflict`, a report's `format=pdf` answers 502
`provider_error`), so `/health` is green while a feature is dead (A130). The defence is the image
check on the host (§ Before deploying), not a monitor — which is why that step is a deployment
requirement rather than a convenience.

## The breach runbook (A349)

**Who decides, and how long we have — the two facts a runbook needs, and they are blanks because
inventing them would be worse than leaving them visible:**

| Fact | Value |
|---|---|
| **Who declares a breach and decides on notification** | `<a name and a second name>` |
| **The notification window** | `<UU PDP's window, confirmed against the regulation at the time — not a number from memory>` |
| **Who is told first, internally** | `<a name>` |
| **The authority, and the channel** | `<the Indonesian data-protection authority's filing channel>` |

The order below matters: **containment before preservation is a decision with a cost**, and doing
them in the wrong order either destroys evidence or leaves the attacker with access while you take
a snapshot. Both are wrong; the order here is the one that accepts losing some evidence in exchange
for stopping the loss.

### 1. Contain — stop the bleeding

1. **Rotate `JWT_SECRET`.** Every session token becomes invalid immediately, so every user is
   signed out. This is the single fastest way to end an attacker's access to accounts, and it is
   why it is first.
2. **Rotate `ENCRYPTION_KEY_HEX` — only if you must.** Rotating it makes every stored ciphertext
   unreadable (BYOK keys, calendar refresh tokens), so it destroys data the attacker may not have
   taken. Do it when the key itself is what leaked, not reflexively; when it is not the leak,
   revoking the sessions and the provider keys below is the containment.
3. **Revoke all sessions and all devices.** `auth_sessions` and `devices` are both revocable; a
   revoked device's token is refused and its cache is wiped on the next pass.
4. **Disable signups** so no new account can be created while the investigation runs.
5. **Rotate every provider credential** the server holds: the model provider key, the search
   service's secret, SMTP, the R2 API token, the QRIS keys, and the push credentials. Rotating
   them is cheap and they are the ones that spend money.
6. **Rotate the updater's minisign key only through a release** (A332): installed apps verify
   against the key compiled into them, so a new key reaches an install only via an update signed
   by the old one. If the old key is what leaked, there is no in-band path — see R8 in
   `docs/THREAT_MODEL.md`.

### 2. Preserve — before rotating anything else

**Snapshot the database and the logs first**, because the steps above destroy evidence: a rotated
`JWT_SECRET` makes the old tokens unverifiable, and a wiped device cache removes what was on it.

```sh
pg_dump -Fc "$DATABASE_URL" > /var/backups/incident/$(date +%Y%m%dT%H%M)-incident.dump
tar czf /var/backups/incident/$(date +%Y%m%dT%H%M)-logs.tgz /var/log/stvdy /var/log/nginx
```

Take the snapshot **off the host** as soon as it is written. Then read the evidence from the
snapshot rather than from the live database, so the investigation cannot change what it is
investigating.

### 3. Assess — what was taken, and from whom

Work from the data inventory in `docs/EULA.md` § 9, which is the same list the privacy notice
publishes. For each data class, answer three questions:

| Data class | Was it accessed? | Whose? | How do you know? |
|---|---|---|---|
| Account email addresses | | | `auth_sessions`, the access logs |
| Uploaded material and extracted text | | | `sources`, the R2 access log |
| Chat content | | | `messages` |
| Calendar events, tasks, settings | | | the `+sync` tables, `calendar_connections` |
| Push tokens and device identifiers | | | `push_tokens`, `devices` |
| Payment records | | | `payments`, the gateway's own records |
| Credentials (password hashes, BYOK keys) | | | `users`, `user_api_keys` |

**A class you cannot answer "no" for is a class you must report as affected.** The temptation is to
report the narrowest plausible scope; the regulation is what decides the scope, and the honest
answer at this stage is the one that says what you do not know.

### 4. Notify — within the window, to the people who are affected

The window is the blank at the top of this section. The template:

> **Subject: A security incident affecting your Stvdy account**
>
> On <date>, we found that <what happened, in plain Indonesian and plain English>. The information
> involved was <the data classes from § 3>. It did not include <what it did not include — only say
> this if § 3 established it>.
>
> What we have done: <the containment steps taken, in the order taken>. What you should do:
> <change your password; re-connect Google Calendar; re-add your API key — whichever the incident
> actually requires, not a generic list>.
>
> We are sorry. If you have questions, write to <the address a human reads>.

Notify **the authority and the affected users**, and notify them once rather than in stages: a
partial notice followed by a correction teaches users to ignore the first one.

### 5. Review — and change something

Within two weeks, write down: how the breach happened, which of the four monitoring signals would
have caught it (and if none would have, that is the finding), what this runbook got wrong under
pressure, and what changed as a result. **A review that produces no change is a review that did not
happen.** Record it in this section with a date.

## Retention (A349)

One row per data class, with **the mechanism that enforces it**. A retention policy with no
mechanism is a sentence, so every row names the thing that actually does the deleting.

| Data class | Window | Enforced by |
|---|---|---|
| `change_log` (the sync history) | **90 days** | the `compact_change_log` job, which trims below the watermark (A19, A165) |
| Deleted accounts and their rows | **immediately for access; the rows are purged 7 days after the request** | `purge_deleted_accounts`, scheduled by `DELETE /account` at `deleted_at + ACCOUNT_DELETION_GRACE_DAYS` (A355), with the scheduler's `pass` as the backstop for a marked account whose job was lost. Access ends at the request — sessions, devices, the ICS feed token and pending credentials are revoked then — so the window is the purge's scheduling rather than a recoverable state |
| Generated documents (`reports/`, `cheatsheets/` in R2) | `<the operator's choice>` | an R2 **lifecycle rule** on those prefixes, which is the only thing that deletes an object nobody asked about again |
| Uploaded sources and their previews | until the student deletes them, or the account is deleted | the deletion paths (`DELETE /sources/:id`, account deletion), plus the R2 lifecycle rule above as the backstop |
| The object-storage bytes an account-deletion purge could not remove | the R2 lifecycle rule's window | the purge collects `sources.file_key` and the `cheatsheets` keys and calls `delete_objects`, which **logs and continues** rather than failing — a purge that refused to complete because R2 was briefly unavailable would leave the account half-deleted, which is worse than an orphaned object. So an object whose delete failed is the accepted residual, and the lifecycle rule on those prefixes is what eventually removes it |
| Chat content, tasks, events, settings | until the student deletes them, or the account is deleted | the deletion paths; the `+sync` tables cascade |
| `var/mail` (the dev sink) | **never in production** | `SMTP_URL` is set in production, so the sink is not written at all. A dev sink left on a host is unencrypted verification and receipt mail at rest, which is why this is a row rather than a note |
| Service logs | `<the operator's window, e.g. 30 days>` | the log rotation on the host. Long enough to investigate a report, short enough not to be a shadow database |
| Payment records | `<the statutory accounting period — B2>` | the `payments` table. **This row is deliberately not 90 days**: tax and accounting rules require these records, and they survive account deletion in anonymised form (see `docs/EULA.md` § 9) |
| `usage_events` and `credit_ledger` | `<the operator's window>` | not yet enforced by a job. Named here so the gap is visible rather than implied — the tables grow monotonically today |

**The row that had no mechanism, and now does.** Deleted accounts were *specified* to be purged
by `purge_deleted_accounts` — the job kind named in `migrations/0015_jobs.sql` — and until A355 no
handler dispatched it: `DELETE /account` and `GET /account/export` did not exist either. **That is
now shipped.** `DELETE /account` marks the account, revokes every credential at once, cancels its
jobs and schedules the purge at `deleted_at + ACCOUNT_DELETION_GRACE_DAYS` (default 7 days); the
scheduler's `pass` enqueues the same job for any marked account whose own job was lost, so the
window is the `run_at` and the two producers cannot disagree about it. The job is idempotent —
`purged_at` is written with `coalesce` and every delete matches nothing on a second pass — which
is what makes a double run a no-op rather than an error.

What the purge deletes and what it keeps is a per-table-group mapping, not a cascade: the
student's content goes, and `payments`, `credit_ledger`, `subscriptions` (cancelled),
`usage_events` and `change_log` survive attached to an **anonymized** `users` row. That row cannot
be deleted at all — `payments.user_id` and `credit_ledger.user_id` are `not null references users
on delete cascade`, so removing it would take the payment ledger with it — which is why the
tombstone is what keeps the financial half of this table. A355a states the full mapping and the
reason for each group.

## Where the launch decision is made

`docs/RELEASE.md` § Launch checklist is the single place the launch decision is recorded. Every row
there is typed **code**, **user** or **blocked**, and the rows that depend on this document are:
row 14 (the restore drill run against the production backup), row 15 (the uptime check and alert
routing live) and row 16 (the breach runbook's two blanks filled in). `docs/superpowers/HANDOVER.md`
is the live list of what the user owes, and it is the authority for those rows.
