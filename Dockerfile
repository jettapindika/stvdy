# syntax=docker/dockerfile:1
#
# The server image. Three native dependencies the code loads at runtime are pinned
# here and nowhere else: Chromium (`CHROME_PATH`, A129), pdfium (`PDFIUM_LIB_PATH`,
# A51/A341) and the embedding model (`FASTEMBED_CACHE_DIR`, A2/A342). Each has a
# documented absent-binary policy, and the failure mode of getting one wrong is a
# feature that quietly stops working on deploy rather than a failed start
# (`docs/RELEASE.md`): no Chromium is a PDF nobody can download, no pdfium is a
# source stuck in `processing`, no model is a first ingest that stalls on a host with
# no egress.
#
# The image therefore carries more than a minimal runtime — the release `target/`
# directory is in it, so the PDF acceptance test can run *inside* the image (A337).
# That is deliberate: `crates/server/tests/pdf.rs` is `#[ignore]`d so a laptop without
# a browser still gets a green `cargo test`, and the only honest way to keep it from
# being a test nobody runs is to run it here, against the image's own Chromium, pdfium
# and assets.

# ---------------------------------------------------------------------------
# Build
# ---------------------------------------------------------------------------
# Trixie, not bookworm, and the floor is `ort-sys`'s prebuilt ONNX Runtime: it is linked
# against glibc ≥ 2.38 (`__isoc23_strtoll`, `__isoc23_strtoull`, `_M_replace_cold`) while
# bookworm carries 2.36, and `ort-sys` 2.0.0-rc.13's `build/download/dist.tsv` carries
# exactly one distribution per target — so there is no lower-glibc build to switch to and
# the base image is what has to move. The runtime stage moves with it: a binary linked
# against trixie's glibc does not load on bookworm.
FROM rust:1.94.1-trixie AS build
WORKDIR /src

# The workspace manifests and the lockfile first, so a source-only change does not
# re-resolve the crate graph. `--locked` at the end means the image builds the
# lockfile CI tested, not a fresh resolution.
COPY Cargo.toml Cargo.lock rust-toolchain.toml ./

COPY crates ./crates
COPY migrations ./migrations
COPY prompts ./prompts
COPY templates ./templates
COPY config ./config
# The print assets. `rust-embed` compiles them into the binary, so a file missing from
# the repository fails the build here rather than 404ing at print time (A339).
COPY assets ./assets

ENV SQLX_OFFLINE=true
# The library and the binary, then the test targets — in that order so a source-only change
# re-uses the dependency layer.
#
# `cargo test --no-run` is what puts the integration-test binaries under
# `target/release/deps/` (`pdf-<hash>` among them). `cargo build` alone does **not** build
# test targets, and without this the image would carry no acceptance test and the whole
# point of copying `target/release/` would be lost: CI would look for `deps/pdf-*` and find
# nothing. It needs no database: this tree has no compile-time query macros, so
# `cargo build` never asks Postgres for a schema.
RUN cargo build --release --workspace --locked \
    && cargo test --release --workspace --no-run --locked

# The embedding model, pre-baked at an absolute path (A342).
#
# `FASTEMBED_CACHE_DIR` defaults to the *relative* `.fastembed_cache`, which in a
# container means "relative to the working directory" — so a deployment that changed
# `WORKDIR` would silently re-download 1.06 GB at the first ingest, on a host that may
# have no egress. Populating it here uses `fastembed`'s own layout through the server's
# own code path, so the cache the image ships is the cache the running process reads;
# a hand-copied directory would be invalidated by a `fastembed` version bump with no
# error until ingest.
ENV FASTEMBED_CACHE_DIR=/opt/models/fastembed
# The `--` is load-bearing: without it cargo claims `--fetch-embedder-model` as its own
# argument and refuses the command outright ("unexpected argument … found"), which is what
# the image build on the staging host died on. Everything after `--` is passed through to
# the binary, and the binary parses `argv[1]` itself (`main.rs`).
RUN cargo run --release -p stvdy-server -- --fetch-embedder-model

# ---------------------------------------------------------------------------
# Runtime
# ---------------------------------------------------------------------------
FROM debian:trixie-slim AS runtime

# The Chromium pin. `chromium` alone tracks Debian's `stable` and the renderer's output
# is the document a student prints — the acceptance test asserts a page count and a
# footer read back from the renderer, so an unpinned browser makes the test's result
# depend on when the image was built (`docs/RELEASE.md`).
#
# `chromium-common` is pinned with it because the two are built from one source and
# Debian's dependency is unversioned: pinning only the binary would let the pair drift.
#
# The value is read from `apt-cache policy chromium` in the trixie archive at the time of
# writing — it lives in `trixie-security`, which is where a browser's current version is,
# while trixie's main archive carries an older one — and `.github/workflows/ci.yml`'s
# `image` job asserts that the version the running image reports equals this string. A
# base-image change that silently moves Chromium therefore fails the build rather than
# changing every PDF.
#
# **This pin has a shelf life, and Debian keeps only the current version.** A superseded
# browser is removed from `trixie-security` rather than kept beside the new one, so once
# Debian ships the next upload this line stops resolving and `apt-get install` fails with
# `Version '…' for 'chromium' was not found` — which is the intended failure: the build stops
# rather than silently rendering every student's PDF with a different browser. The remedy is
# a re-pin plus a re-run of the acceptance test (`scripts/release-image.sh`), and the
# supersede is frequent enough that this is a weekly-rebuild chore rather than an incident
# (A337). It happened during this deployment: `154.0.8037.92-1~deb13u1` was current when the
# first image was built and `155.0.8059.39-1~deb13u1` replaced it hours later.
ARG CHROMIUM_VERSION=155.0.8059.39-1~deb13u1

# LibreOffice is for the PPTX/DOCX preview (A158, A343). The three packages rather than
# the whole suite: `--convert-to pdf` needs the writer and the impress filters.
#
# `fonts-liberation` is what the template's font stack falls back to (`Inter, Arial`);
# the typefaces the template actually names are the vendored ones in `assets/`.
#
# **`chromium-sandbox` is not optional and is not pulled in by `chromium`.** Debian ships
# the setuid sandbox helper as its own package, and without it Chromium refuses to start:
# `No usable sandbox! If this is a Debian system, please install the chromium-sandbox
# package to solve this problem.` That is A338's exact scenario — the sandbox is required
# outside dev and `--no-sandbox` is not the answer — so the package the sandbox needs is
# part of the image, pinned to the same version as the browser it belongs to. Its absence is
# not a failed render but a **boot that hangs**: the browser dies immediately, the renderer
# waits for a browser that is gone, and the server never binds its port. Found by
# `scripts/release-image.sh` on the first image whose sandbox was ever exercised.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl fonts-liberation \
      "chromium=${CHROMIUM_VERSION}" "chromium-common=${CHROMIUM_VERSION}" \
      "chromium-sandbox=${CHROMIUM_VERSION}" \
      libreoffice-core libreoffice-writer libreoffice-impress \
    && rm -rf /var/lib/apt/lists/*

ENV CHROME_PATH=/usr/bin/chromium

# pdfium, at the release this repository's own test names (`chromium/8086`,
# `crates/server/tests/pdfium.rs`) and verified against a checksum committed here rather
# than fetched-and-trusted: an unpinned download from a release URL is a supply-chain
# hole in the one dependency that parses attacker-supplied files (A341).
#
# `sha256sum -c` before the tarball is unpacked, so a substituted artifact is a build
# failure and not a library the server loads at boot.
#
# **Unpacked with its own `lib/` and `include/` directories, no `--strip-components`.** The
# asset's layout is `lib/libpdfium.so` plus `include/*.h` at the top level, so stripping one
# component moves the library to `/opt/pdfium/libpdfium.so` and leaves `PDFIUM_LIB_PATH`
# naming a file that does not exist — which A51 makes a **boot** failure rather than a
# disabled extractor. It is a one-line mistake that the image's own check catches and a
# reading of the Dockerfile does not, which is why the layout is stated here.
ARG PDFIUM_RELEASE=chromium/8086
ARG PDFIUM_SHA256=588577cf52dabc1a444988bac841920df54cc2f141801424de97ab04f4fbb935
RUN curl -sSL -o /tmp/pdfium.tgz \
      "https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_RELEASE}/pdfium-linux-x64.tgz" \
    && echo "${PDFIUM_SHA256}  /tmp/pdfium.tgz" | sha256sum -c - \
    && mkdir -p /opt/pdfium \
    && tar xz -C /opt/pdfium -f /tmp/pdfium.tgz \
    && rm /tmp/pdfium.tgz

ENV PDFIUM_LIB_PATH=/opt/pdfium/lib/libpdfium.so

# The embedding model, from the build stage. `COPY --from` rather than a second download,
# so the image's cache is byte-identical to the one the build verified.
COPY --from=build /opt/models/fastembed /opt/models/fastembed
ENV FASTEMBED_CACHE_DIR=/opt/models/fastembed

# The vendored print assets, for the release engineer.
#
# These are **already in the binary**: `rust-embed` compiled them in the build stage, and
# `GET /assets/{*path}` serves that copy. This directory is the second, redundant copy
# A339 describes — an operator who wants to see what shipped, and to check it against
# `assets/checksums.txt`, should not have to unpack the binary. The redundancy is stated
# in `docs/ARCHITECTURE.md` so it is not mistaken for a mistake.
COPY assets /opt/assets
ENV ASSETS_DIR=/opt/assets

# The two configuration files, which are the **only** repository files the running process
# reads from disk. Everything else it needs is compiled in — `prompts/` and `templates/` are
# `include_str!` (`crates/core/src/prompts/registry.rs`, `crates/core/src/render/mod.rs`,
# `crates/server/src/reports/render.rs`) and `migrations/` is `sqlx::migrate!` — which is
# exactly why this COPY is easy to leave out and fatal when it is.
#
# `AppConfig::from_env` resolves `PLANS_CONFIG` and `BILLING_POLICY_CONFIG` against the
# process's working directory (`config.rs`), and `WORKDIR` here is `/app`, so without this the
# server answers `loading plans config at /app/config/plans.toml: No such file or directory`
# and **exits** — a boot failure, not a degraded feature, and it takes `--migrate-only` with it
# so the deployment cannot even migrate. Found by `scripts/release-image.sh` on the first image
# that ever got this far.
COPY config /app/config

# A non-root user, because Chromium's sandbox is what the renderer requires outside dev
# (`crates/server/src/render/chrome.rs`: `allow_no_sandbox` is `cfg.env.is_dev()`) and
# running as root with `--no-sandbox` is explicitly not the answer (A338). The UID is
# fixed so a mounted volume has predictable ownership.
#
# The sandbox also needs a writable `/tmp` and enough `/dev/shm`; both are deployment
# flags (`--tmpfs /tmp`, `--shm-size`), recorded in `docs/OPERATIONS.md` because a
# missing `/dev/shm` is the classic "Chromium dies with a memory error that looks like a
# code bug" failure.
RUN useradd --create-home --uid 10001 --shell /usr/sbin/nologin stvdy

COPY --from=build /src/target/release/stvdy-server /usr/local/bin/stvdy-server
# The acceptance test's binary, and nothing else from the build.
#
# The image carries `crates/server/tests/pdf.rs` so the one test that needs a browser can run
# **inside** the image against the image's own Chromium, pdfium and print assets — that is
# the whole point of the copy (A337), and it needs exactly one binary: the test builds its
# document from `stvdy_core` (compiled in), serves the embedded assets itself on a loopback
# port, and reads no fixture from disk. Verified by reading `tests/pdf.rs`, which has no
# `include_str!`, no `File::open` and no `CARGO_MANIFEST_DIR`.
#
# **Copying the whole `target/release/` instead was 6.4 GB of dead weight** — sixty-odd other
# test binaries, their rlibs and cargo's `build/` and `.fingerprint/` bookkeeping, none of
# which anything in the image runs. It also doubled the disk the build needs, because
# BuildKit holds the build stage's copy and the runtime image's copy at once, and that peak
# is what made this image unbuildable on a 45 GB host. `deps/*.so` comes along because the
# test links against the build's shared objects (`libpdfium_render-*.so` among them).
COPY --from=build /src/target/release/deps/pdf-* /opt/tests/deps/
COPY --from=build /src/target/release/deps/*.so /opt/tests/deps/

USER stvdy
WORKDIR /app

ENV APP_ENV=prod \
    BIND_ADDR=0.0.0.0:8080 \
    SOFFICE_BIN=/usr/bin/soffice

EXPOSE 8080

# The migration step is the same entrypoint with `--migrate-only`, and it must run to
# completion before any replica serves traffic (A346, `docs/OPERATIONS.md`).
ENTRYPOINT ["/usr/local/bin/stvdy-server"]
