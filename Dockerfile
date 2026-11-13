# Stage 1: build
# The repo's pinned Rust (rust-toolchain.toml, 1.99.1): the image builds with
# the same compiler as every local build and test run. rust:1.99.1-slim is
# Debian trixie (glibc 2.41), the runtime stage's release.
FROM rust:1.99.1-slim AS builder
WORKDIR /build
RUN apt-get update && apt-get install -y --no-install-recommends \
    pkg-config libssl-dev ca-certificates wget && rm -rf /var/lib/apt/lists/*
COPY litegen-core/Cargo.toml litegen-core/Cargo.lock litegen-core/
COPY litegen-core/src litegen-core/src
COPY litegen-core/migrations litegen-core/migrations
COPY models models
WORKDIR /build/litegen-core
# --locked, like every cargo step in CI: the image gets exactly Cargo.lock's
# versions, and a lock that no longer matches Cargo.toml fails the build
# instead of being re-resolved inside the image.
RUN cargo build --locked --release --bin litegen

# Stage 2: runtime
# Must match the builder's Debian release (trixie) so glibc versions line up —
# a bookworm runtime (glibc 2.36) can't run a binary built on trixie (glibc 2.41).
FROM debian:trixie-slim
RUN apt-get update && apt-get install -y --no-install-recommends \
    ca-certificates sqlite3 wget && rm -rf /var/lib/apt/lists/*
WORKDIR /app
COPY --from=builder /build/litegen-core/target/release/litegen /app/litegen
COPY models /app/models
# /app/data holds what litegen keeps: its SQLite database (/app/data/litegen.db,
# the default when no setting names a database) and the kept results (meshes,
# videos, images, published references) under /app/data/assets. The
# unprivileged user below owns it, and it is a volume, so both outlive the
# container. litegen refuses to start when it cannot write there.
RUN mkdir -p /app/data/assets && chown -R 1000:1000 /app/data
VOLUME ["/app/data"]
# The image sets none of litegen's own settings, so nothing here outranks a
# config file: environment variables (-e) beat a mounted /app/litegen.yaml (or
# the file --config or LITEGEN_CONFIG_FILE_PATH names), which beats the
# built-in defaults (0.0.0.0, port 4000, /app/data/litegen.db, /app/data/assets).
ENV LITEGEN_MODELS_DIR=/app/models
# Set LITEGEN__SERVER__PUBLIC_BASE_URL to the origin clients reach this
# container on (e.g. https://api.example.com). 3D asset URLs are minted
# absolute from it; the default derives from the bind host, which inside a
# container is the unreachable 0.0.0.0 (loopback after the fallback).
# ENV LITEGEN__SERVER__PUBLIC_BASE_URL=https://api.example.com
EXPOSE 4000
USER 1000:1000
ENTRYPOINT ["/app/litegen"]
