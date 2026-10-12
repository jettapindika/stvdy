# Operations

The day-two runbook. `docs/RELEASE.md` is the normative release document — what is
distributed, under what policy, with which pinned components; this file is how an operator
runs and checks the server that ships from it.

Written by Phase 15's image work (Tasks 6–9). The later sections this document is planned to
carry — backups and the restore drill, monitoring thresholds, the breach runbook, retention
windows — are Phase 15 Task 13's and are not written yet. They are absent rather than stubbed,
so an empty heading is not mistaken for a procedure someone forgot to fill in.

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
