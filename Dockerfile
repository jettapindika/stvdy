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
FROM rust:1.94.1-bookworm AS build
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
RUN cargo run --release -p stvdy-server --fetch-embedder-model

# ---------------------------------------------------------------------------
# Runtime
# ---------------------------------------------------------------------------
FROM debian:bookworm-slim AS runtime

# The Chromium pin. `chromium` alone tracks Debian's `stable` and the renderer's output
# is the document a student prints — the acceptance test asserts a page count and a
# footer read back from the renderer, so an unpinned browser makes the test's result
# depend on when the image was built (`docs/RELEASE.md`).
#
# `chromium-common` is pinned with it because the two are built from one source and
# Debian's dependency is unversioned: pinning only the binary would let the pair drift.
#
# The value is read from `apt-cache policy chromium` in the bookworm archive at the time
# of writing, and `.github/workflows/ci.yml`'s `image` job asserts that the version the
# running image reports equals this string. A base-image change that silently moves
# Chromium therefore fails the build rather than changing every PDF.
ARG CHROMIUM_VERSION=154.0.8037.92-1~deb12u1

# LibreOffice is for the PPTX/DOCX preview (A158, A343). The three packages rather than
# the whole suite: `--convert-to pdf` needs the writer and the impress filters.
#
# `fonts-liberation` is what the template's font stack falls back to (`Inter, Arial`);
# the typefaces the template actually names are the vendored ones in `assets/`.
RUN apt-get update && apt-get install -y --no-install-recommends \
      ca-certificates curl fonts-liberation \
      "chromium=${CHROMIUM_VERSION}" "chromium-common=${CHROMIUM_VERSION}" \
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
ARG PDFIUM_RELEASE=chromium/8086
ARG PDFIUM_SHA256=588577cf52dabc1a444988bac841920df54cc2f141801424de97ab04f4fbb935
RUN curl -sSL -o /tmp/pdfium.tgz \
      "https://github.com/bblanchon/pdfium-binaries/releases/download/${PDFIUM_RELEASE}/pdfium-linux-x64.tgz" \
    && echo "${PDFIUM_SHA256}  /tmp/pdfium.tgz" | sha256sum -c - \
    && mkdir -p /opt/pdfium \
    && tar xz -C /opt/pdfium --strip-components=1 -f /tmp/pdfium.tgz \
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
# The release profile's whole output, which is what carries the integration-test binaries
# (`deps/pdf-<hash>`, `deps/release-<hash>`, …). The acceptance test runs from here.
COPY --from=build /src/target/release/ /opt/tests/

USER stvdy
WORKDIR /app

ENV APP_ENV=prod \
    BIND_ADDR=0.0.0.0:8080 \
    SOFFICE_BIN=/usr/bin/soffice

EXPOSE 8080

# The migration step is the same entrypoint with `--migrate-only`, and it must run to
# completion before any replica serves traffic (A346, `docs/OPERATIONS.md`).
ENTRYPOINT ["/usr/local/bin/stvdy-server"]
