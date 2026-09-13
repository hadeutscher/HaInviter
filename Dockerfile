# syntax=docker/dockerfile:1

# ============================================================================
# HaInviter — Alpine runtime image
#
# The server is cross-compiled to statically linked musl so the final image is
# Alpine with nothing else in it: no libc to keep in step, no interpreter, no
# package manager surface. The build itself happens on Debian, because the
# Dioxus CLI is only published as a glibc binary and building it from source
# would drag in an image-codec C++ toolchain we have no other use for.
#
# Images are built one architecture at a time on a native runner, so the musl
# target always matches the host and plain `musl-gcc` can compile the bundled
# SQLite.
# ============================================================================

FROM rust:1.98-trixie AS builder

ARG DX_VERSION=0.7.10
# Supplied by BuildKit: "amd64" or "arm64".
ARG TARGETARCH

WORKDIR /src

RUN apt-get update \
    && apt-get install -y --no-install-recommends musl-tools musl-dev \
    && rm -rf /var/lib/apt/lists/*

# Resolve the musl triple once and keep it on disk for the later stages.
RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) triple=x86_64-unknown-linux-musl ;; \
      arm64) triple=aarch64-unknown-linux-musl ;; \
      *) echo "unsupported architecture: $TARGETARCH" >&2; exit 1 ;; \
    esac; \
    echo "$triple" > /musl-target; \
    rustup target add "$triple"; \
    rustup target add wasm32-unknown-unknown

RUN set -eux; \
    case "$TARGETARCH" in \
      amd64) arch=x86_64 ;; \
      arm64) arch=aarch64 ;; \
    esac; \
    curl -fsSL -o /tmp/dx.tar.gz \
      "https://github.com/DioxusLabs/dioxus/releases/download/v${DX_VERSION}/dx-${arch}-unknown-linux-gnu.tar.gz"; \
    tar -xzf /tmp/dx.tar.gz -C /usr/local/bin dx; \
    rm /tmp/dx.tar.gz; \
    dx --version

COPY . .

# `dx bundle` builds the WASM client and the server binary and flattens both
# into --out-dir: the binary as `server`, the client next to it in `public/`.
# CI=true keeps the CLI non-interactive and builds the two halves in sequence,
# which keeps peak memory within a hosted runner.
ENV CI=true
RUN set -eux; \
    triple="$(cat /musl-target)"; \
    upper="$(echo "$triple" | tr 'a-z-' 'A-Z_')"; \
    lower="$(echo "$triple" | tr - _)"; \
    export "CC_${lower}=musl-gcc"; \
    export "CARGO_TARGET_${upper}_LINKER=musl-gcc"; \
    dx bundle --release --debug-symbols=false --out-dir /out @server --target "$triple"; \
    test -x /out/server; \
    test -d /out/public

# ── Runtime ────────────────────────────────────────────────────────────────
FROM alpine:3.22

# Nothing is installed: the binary is statically linked and HaInviter never
# makes an outbound connection, so it needs neither a libc nor a CA bundle.
RUN addgroup -S -g 10001 hainviter \
    && adduser -S -u 10001 -G hainviter -H -D hainviter \
    && mkdir -p /data \
    && chown 10001:10001 /data

COPY --from=builder /out /opt/hainviter

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
