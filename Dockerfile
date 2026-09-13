# syntax=docker/dockerfile:1

# ============================================================================
# HaInviter
#
# Builder and runtime are both Debian trixie, so the server binary links
# against the glibc it was built against and no cross-compilation is involved:
# `dx bundle` runs exactly as it does locally.
#
# An earlier version of this file targeted a statically linked musl binary on
# an Alpine runtime. Getting there meant passing our own `@server --target` to
# dx, which replaces the per-half feature sets dx infers from the Cargo.toml
# feature names — and it shipped a "server" binary built with the *web*
# features: no database, no routes, and a panic inside wasm-bindgen on the
# first line of main. A ~25 MB runtime image is a good trade for not doing
# that.
# ============================================================================

FROM rust:1.98-trixie AS builder

ARG DX_VERSION=0.7.10
# Supplied by BuildKit: "amd64" or "arm64".
ARG TARGETARCH

WORKDIR /src

# The Dioxus CLI, as a prebuilt binary: building it from source would pull in
# an image-codec C++ toolchain we have no other use for.
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) arch=x86_64 ;; \
      arm64) arch=aarch64 ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    curl -fsSL -o /tmp/dx.tar.gz \
      "https://github.com/DioxusLabs/dioxus/releases/download/v${DX_VERSION}/dx-${arch}-unknown-linux-gnu.tar.gz"; \
    tar -xzf /tmp/dx.tar.gz -C /usr/local/bin dx; \
    rm /tmp/dx.tar.gz; \
    dx --version

COPY . .

# Builds the WASM client and the native server and puts both in the crate's
# dist/: the binary as `server`, the client beside it in `public/`. CI=true
# keeps the CLI non-interactive and builds the two halves in sequence, which
# keeps peak memory within a hosted runner.
#
# No feature flags and no target: dx infers both from the Cargo.toml, and
# overriding either is what broke the first version of this image.
ENV CI=true
RUN set -eux; \
    dx bundle --package hainviter --release --debug-symbols=false; \
    test -x dist/server; \
    test -f dist/public/index.html

# ── Runtime ────────────────────────────────────────────────────────────────
FROM debian:trixie-slim

# Nothing is installed on top of the base: HaInviter never opens an outbound
# connection, so it needs no CA bundle, and it reads no timezone database.
RUN set -eux; \
    groupadd --system --gid 10001 hainviter; \
    useradd --system --uid 10001 --gid 10001 --no-create-home hainviter; \
    mkdir -p /data; \
    chown 10001:10001 /data

COPY --from=builder /src/dist /opt/hainviter

# Everything that must not be lost lives under /data: mount it from the host.
ENV HAINVITER_DATA_DIR=/data \
    IP=0.0.0.0 \
    PORT=8080
VOLUME ["/data"]
EXPOSE 8080

USER 10001:10001
# The server resolves the client bundle relative to its own directory.
WORKDIR /opt/hainviter
ENTRYPOINT ["/opt/hainviter/server"]
