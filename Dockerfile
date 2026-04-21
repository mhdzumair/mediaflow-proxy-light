# ---------------------------------------------------------------------------
# Multi-stage Dockerfile — builds from source.
#
# Multi-arch (linux/amd64 + linux/arm64) is handled by Docker buildx.  Under
# QEMU the arm64 build is slower, but this lets the Docker job run in
# parallel with the desktop/mobile binary builds in CI without any
# cross-job artifact choreography.
#
# TLS backend: `tls-rustls` (the crate default) → no system OpenSSL needed.
# ---------------------------------------------------------------------------

FROM rust:1.95-slim-bookworm AS builder

WORKDIR /usr/src/app

# Build deps. Beyond pkg-config + build-essential, the `extractors` feature
# pulls in `rquest` → `boring-sys2` which statically compiles BoringSSL:
#   * cmake / ninja / perl / python3 / golang-go — BoringSSL build system
#   * git                                         — boring-sys2 invokes
#                                                   `git init` + `git apply`
#                                                   to apply bundled patches
#   * clang + libclang-dev                        — bindgen needs libclang.so
#     to generate BoringSSL FFI bindings
#
# The CI release workflow builds on native runners (see .github/workflows/
# release.yml) that already ship all of the above, which is why this
# Dockerfile has historically gotten away with a shorter list. A plain
# `docker build .` without these panics inside the boring-sys2 build script.
RUN apt-get update && apt-get install -y --no-install-recommends \
        pkg-config \
        build-essential \
        cmake \
        ninja-build \
        perl \
        python3 \
        golang-go \
        git \
        clang \
        libclang-dev \
        && rm -rf /var/lib/apt/lists/*

# Dependency-only prebuild for layer caching. `crates/` is needed because
# Cargo.toml has `[patch.crates-io] os_info = { path = "crates/os_info_stub" }`
# — without it, cargo fails to resolve the workspace patch entry.
COPY Cargo.toml Cargo.lock ./
COPY crates ./crates
RUN mkdir src && \
    echo "fn main() {}" > src/main.rs && \
    echo "pub fn add(l: usize, r: usize) -> usize { l + r }" > src/lib.rs && \
    cargo build --release && \
    rm -rf src target/release/deps/mediaflow*

# Real build. `static/` is embedded into the binary by rust-embed when the
# `web-ui` feature is enabled (default), so it must be present at compile
# time even though nothing references it at runtime.
COPY src    ./src
COPY tools  ./tools
COPY static ./static
RUN cargo build --release

# ---------------------------------------------------------------------------
# Runtime — distroless (glibc only, no shell/apt/etc.)
# ---------------------------------------------------------------------------
FROM gcr.io/distroless/cc-debian12

WORKDIR /app

COPY --from=builder /usr/src/app/target/release/mediaflow-proxy-light /app/
COPY config-example.toml /app/config.toml

ENV RUST_LOG=info
ENV CONFIG_PATH=/app/config.toml

EXPOSE 8888

ENTRYPOINT ["/app/mediaflow-proxy-light"]
